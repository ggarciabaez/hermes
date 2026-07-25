# Hermes — F1TENTH Rust Rewrite: Project Plan & Roadmap

**Project name: Hermes** (messenger god — fits a project built entirely around message-passing over Zenoh). **Debug/visualization tool name: Achilles** (Zeno's paradox — always closing the gap on live system state — and Achilles' heel, since the tool's job is surfacing the one vulnerable spot: faults, out-of-bounds params, silent nodes). Note: the Zenoh topic namespace stays `f1tenth/...` since it's tied to the hardware platform/competition class, not the codebase name — don't conflate the two.

**Goals:** (1) learn Rust/async properly through a real project, (2) replace a monolithic GIL-bound Python stack with a decoupled, Zenoh-based node architecture. Sister project: a future VLA project, which may reuse some crates/patterns from here but has no LLM/VLA scope in this project.

**Status note:** the physical robot is currently non-functional (snapped suspension, Jetson Orin Nano not powering on). This project now targets a simulation-first workflow: every node's *interface* (topics, RPC shapes) is designed against real hardware, but the *backend* for sensors/actuators is a Sim Physics Engine node until hardware is repaired. Driver nodes do not change shape between sim and real — only their ingestion source does.

---

## 1. Architecture Overview

**Bus:** Zenoh is the only inter-node transport. Two communication patterns, chosen deliberately per use case:
- **pub/sub** — streaming/telemetry data, commands, anything continuous or fire-and-forget (LiDAR scans, IMU, Twist commands, manual override commands, mode changes, fault reports).
- **queryable/get (RPC)** — anything request/reply shaped: inference, scan-matching solves, MPC solves, param get/set/list, manifests, map snapshots. This is the "microservices" mechanism — the caller doesn't know or care if the service is local or off-board.

**Priority is used at both ends of the spectrum, deliberately:**
- The gamepad → State Machine → safety-critical branches use highest Zenoh priority + `Drop` congestion control (never block behind bulk traffic; a dropped estop is retried, a queued one is late).
- Achilles's subscriptions use background priority + best-effort — it can safely listen to everything precisely because it's configured to never contend with control traffic.

**Language boundary:** Zenoh has first-class Python bindings, so any node that's painful or pointless in Rust (vendor SDKs, fussy hardware bring-up) can be Python without breaking the architecture. This isn't a compromise — a node's language is an implementation detail behind the bus. Currently: OAK-D Bridge only, and deferred (no current use case).

**Discovery & introspection:** every node declares a Zenoh liveliness token at startup (`f1tenth/liveliness/<node_name>`), retracted on clean shutdown or session death — this doubles as crash detection. Every node also implements an `Introspectable` trait (types crate) exposing a manifest queryable describing what it publishes/subscribes/serves, with type names. Achilles builds its graph from liveliness + manifests, never from snooping actual message payloads. Zenoh's admin/stats space supplements this with raw throughput numbers where useful.

**Runtime:** `tokio` powers every non-embedded node. Async concurrency replaces the GIL-bound monolith — each node polls Zenoh, sensors, and does its own work independently.

---

## 2. Node List

| Node | Language | Pattern | Responsibility |
|---|---|---|---|
| Zenoh Router | — | infra | Central router process on the compute box / network |
| Constants | Rust (+ config file) | one-shot `get` | Fixed, non-runtime-mutable values only (wheelbase, max steering, topic namespace, calibration). No live subscription — nothing changes after boot |
| Params Registry | Rust | sub (mirrors all `changed` topics) + queryable (`get-all`/`list`) | Non-authoritative cache of every node's live params, for single-point querying and dashboards. Also dispatches boot-time config load by issuing `SetParam` RPCs to each owning node — same mutation path the gamepad uses, no special loader |
| Gamepad Adapter | Rust | pub (+ queryable client) | Owns gamepad crate + triggers; publishes `Command` → `f1tenth/commands/manual`; hard EStop trigger bypasses all policy, goes straight to State Machine; other triggers fire `SetParam` RPCs |
| State Machine | Rust | pub (`state/mode`) + queryable (`mode/get`, late-joiners) + sub (`safety/report`) | Sole authoritative writer of robot mode: `Manual`, `Auto`, `SLAM`, `EStop`, `SWStop`, `Halt`. Applies severity policy to fault reports (Critical→EStop, Warning→SWStop or forced Manual, Info→log only); logs every transition centrally |
| Safety Monitor | Rust | sub (scan/pose) + pub (`safety/report`) | Independent watchdog for imminent collision / hazard conditions; requests mode changes via fault-report channel — never writes `state/mode` directly |
| Odometry | Rust | sub (VESC telemetry, IMU) + pub (`state/odom`) | Fuses wheel/VESC speed + steering + IMU into a pose estimate |
| SLAM | Rust | sub (scan, odom) + pub (`map/occupancy_grid`) + queryable (`map/get`) | Builds map + pose while in SLAM mode; late-joiners fetch current map via queryable, not via Constants |
| Localization | Rust | sub (scan, odom, map) + pub (`state/pose`) | AMCL-style pose estimate against an existing static map, active in Auto mode |
| Global Planner | Rust | pub/queryable (`plan/waypoints`) | Turns a map/route into a reference path for Controller to track |
| Controller (Planning/Control) | Rust | sub + pub (+ queryable client/server) | Arbitrates only within manual-vs-autonomous *tracking behavior* (mode itself comes from State Machine); runs Pure Pursuit / MPC (nalgebra + clarabel); publishes `Twist` at ~50Hz; owns its own `Tunable` gains |
| VESC Driver | Rust | sub (motor cmd) + pub (telemetry) | Real: serial+JSON to VESC. Sim: subscribes to Sim Engine's internal motor-state topic. Canonical output topics identical either way |
| RPLidar Driver | Rust | pub (scan) | Real: RPLidar SDK. Sim: subscribes to Sim Engine's internal scan topic |
| IMU Driver | Rust | pub (imu) | Real: IMU peripheral. Sim: subscribes to Sim Engine's internal imu topic |
| Sim Physics Engine | Rust | sub (motor cmd) + pub (internal sim/* topics) | Runs its own physics tick independent of query rate; simulates motor/steering response and produces synthetic scan/imu/telemetry consumed by the three driver nodes above. Not on canonical sensor topics directly — drivers are the adapter layer |
| OAK-D Bridge | Python (likely) | pub | Runs DepthAI pipeline; publishes RGB/depth/IMU topics. Deferred — no current use case, adds a 3D dimension not currently needed |
| Inference service (SAC / ONNX) | Rust (`ort`) | queryable | Batch-1 CPU inference, called by Controller when needed |
| Embedded peripherals | Rust (embassy) | serial bridge → pub/sub | LEDs/indicators, possibly backup IMU; peripherals-only scope, lowest priority, real-hardware-only (no sim backend needed) |
| Serial Bridge (host-side) | Rust | pub/sub | Talks framed serial (postcard+COBS) to MCU, republishes onto Zenoh |
| Achilles (Network Debug/Visualization Tool) | Rust (egui, native binary) | sub (everything, best-effort) + queryable client | Standalone tool, separate binary, own Zenoh session. Graph view of live nodes/topics via liveliness+manifest discovery, live stats, and param tweak UI going through the same validated `SetParam`/mode-request paths as any other caller. Built early — useful for visualizing everything built afterward. Generically useful beyond this project given Zenoh's growing role as a DDS/ROS2 alternative |
| Sim Frontend | Rust (egui, native binary) | sub + pub | Visual sim front-end, built after Achilles and reusing its Zenoh/egui bootstrap plumbing where practical, but a separate binary |

Scan matching (ICP) remains plain code inside whichever node needs it (currently telemetry/SLAM) rather than a standalone service/crate. `nalgebra::Isometry2`/`Isometry3` cover SE(2)/SE(3) transform math directly.

**Open item carried over:** confirm whether the OAK-D Lite unit has a working IMU before finalizing whether the embedded layer still owns IMU duty (moot while OAK-D is deferred).

---

## 3. Mode / State Machine

`Mode` enum: `Manual`, `Auto`, `SLAM`, `EStop`, `SWStop`, `Halt`.

**State Machine owns `f1tenth/state/mode` exclusively.** No other node ever publishes to it. Two distinct entry paths, deliberately different in character:

- **Direct (unconditional):** the gamepad's hard EStop trigger goes straight to State Machine and forces `EStop` immediately, no policy evaluation, no queueing behind other logic. This is the human-override path and stays as fast and simple as possible — it's on the highest-priority Zenoh path discussed earlier, with `biased;` ordering in every control node's `select!` loop.
- **Mediated (policy-evaluated):** any node — primarily Safety Monitor, but not exclusively — can publish a fault report to `f1tenth/safety/report` with a severity (`Info` / `Warning` / `Critical`) and message. State Machine evaluates severity against policy (Critical → `EStop`, Warning → `SWStop` or forced `Manual`, Info → logged only, no transition) and logs every report centrally regardless of outcome. This gives one place to see every fault the system has ever raised, instead of scattered per-node logging.

**`Halt`** is qualitatively different from the other modes — it's not an operating mode, it's "the system is shutting down." Triggered by an explicit operator/gamepad shutdown command or a relayed `ctrl_c`. When State Machine publishes `Halt`, every control node treats it exactly like its own `ctrl_c` branch in `NODE_GUIDE.md`'s shutdown section — stop actuators, close resources, flush, break the loop. This unifies what was previously a purely local, per-node shutdown concern with a centrally-triggerable one, without adding a second shutdown mechanism to learn.

**Every control node subscribes to `state/mode`** and treats it as one of its highest-priority `select!` branches — mode is read, not polled.

---

## 4. Params Architecture

Unchanged in spirit from the original decision, refined with two additions:

- **Fixed constants** (wheelbase, max steering angle, topic namespace, calibration): loaded once at startup from the Constants node's one-shot `get`. Never live-subscribed. No late-joiner problem.
- **Tunable parameters** (PID gains, MPC weights, pursuit lookahead, safety limits like max velocity): owned and validated by the node that uses them — not centralized. Validation bounds are declarative, defined directly in each node's `config.toml` section alongside the value:

  ```toml
  [controller.pp]
  lookahead = 0.25
  bounds = [0.1, 0.5]
  ```

  For bounds that are dynamic (context-dependent rather than a fixed range), the owning node submits a fault report to Safety Monitor's channel (mainly for logging/warning) and reverts the param to its last valid value — using the exact same fault-report path as any other safety concern, no special mechanism.
- Each param-owning node still exposes `get`/`set`/`list` queryables and publishes `changed` on mutation.
- **Params Registry node** (new): subscribes to every node's `changed` topic and mirrors values into a single read-only cache, exposing one `list`/`get-all` queryable for dashboards and Achilles. It is never authoritative and never a write path — mutations always go to the owning node directly. It also owns boot-time config loading: reads `config.toml` and dispatches `SetParam` RPCs to each owning node, using the same path a gamepad trigger would use, so there is exactly one way params ever change.

This still mirrors ROS2's per-node parameter-server pattern rather than a centralized store, while solving the actual pain points (easy querying, easy config loading) that motivated reconsidering it.

---

## 5. Network Debug / Visualization Tool

Standalone egui binary, own Zenoh session, built early in the roadmap — before most control nodes exist in final form, since it's the tool used to observe everything built afterward, and before the Sim Frontend, which will reuse its plumbing.

- **Discovery:** liveliness tokens (`f1tenth/liveliness/<node>`) give live node presence/crash detection. The `Introspectable` manifest queryable gives semantic edges (topic name + type) for the graph — no raw message snooping needed to infer topology. Zenoh admin/stats space supplements with throughput numbers.
- **Ingestion model:** does *not* follow `NODE_GUIDE.md`'s single-`select!`-loop pattern (see Section 6, `NODE_GUIDE.md` rework, below) — it's a dynamic aggregator, not a control node.
- **QoS:** subscriptions run at background priority / best-effort, so broad access never contends with control traffic. High-volume raw topics (scans, camera if OAK-D ever lands) are opt-in toggles in the UI rather than always-on.
- **Writes:** live param tweaks and mode-change requests go through the exact same validated queryables/fault-report channel any other caller uses — no privileged bypass. RPC calls include a `source` field so logs distinguish "gamepad set this" from "Achilles set this."
- **Reuse:** worth extracting as a general Zenoh network debugger beyond this project, given Zenoh's growing role as a DDS/ROS2 alternative — add to Section 7's extractable-crates list.

---

## 6. `NODE_GUIDE.md` Rework (see companion doc)

The original guide assumed every node fits the "one `select!` loop, fixed known inputs" model. Two real exceptions have emerged and need to be documented as sanctioned patterns, not violations:

1. **Long CPU-bound solves** (MPC/clarabel) — offloaded via `spawn_blocking`, raced against higher-priority `select!` branches (mode/estop), result ignored if preempted.
2. **Aggregator/tool nodes** (Achilles, and anything like it later) — dynamic, unbounded set of inputs, no ordering guarantee needed, "latest value wins" is correct. These use background tasks feeding shared state read once per UI frame, explicitly *not* forced into the single-loop model.

See the updated `NODE_GUIDE.md` for the full writeup.

---

## 7. Tooling Summary

**Comms & async**
- `zenoh` — pub/sub + queryable/RPC bus, liveliness tokens, admin/stats space
- `tokio` — async runtime for all non-embedded nodes
- `postcard` — compact serde-compatible binary format for RPC/serial payloads
- `serde` (+ derive) — shared across all typed payloads
- `egui` / `eframe` — native GUI for Achilles and (later) the sim frontend

**Perception / planning**
- `nalgebra` — linear algebra, coordinate transforms, kinematics
- `clarabel` — interior-point QP solver for MPC
- `kornia-rs` or `imgproc`/`image` — any real-time Rust-side CV (deferred alongside OAK-D)
- `ort` (ONNX Runtime bindings) — inference for SAC net / scan-matching nets exported from Python

**Embedded**
- `embassy-rp` / `embassy-esp32` — async HAL for peripherals
- `postcard` + COBS — framing for serial link, no zenoh-pico/FFI needed given current MCU scope

**Dropped / deferred**
- `candle-core`/`candle-nn` — not needed without on-device LLM training/inference
- `opencv` (Rust bindings) — avoid; build/link pain not justified given CV scope
- `zenoh-pico` via FFI/bindgen — deferred, only worth it for a future simpler-robot project
- OAK-D Bridge — deferred, no current use case for the camera

---

## 8. Crates/Tools Worth Extracting (Reusable Beyond This Project)

1. **Types/messages crate** (`f1tenth-types`) — every cross-node type: `Command`, `SetParam`/`GetParam`/`ListParams`, fault-report types, `Mode` enum, telemetry structs, `Pose`/`Twist` newtypes, topic-name constants, `Tunable` trait, `Introspectable` trait. Highest priority; everything depends on it.
2. **Gamepad-trigger crate** — input → `Command`/`SetParam` mapping, project-agnostic.
3. **Zenoh RPC helper** — thin wrapper over `get`/queryable registration with `postcard` serialization, typed request/response, timeout handling.
4. **Serial framing helper** (postcard+COBS) — reusable if any MCU work continues.
5. **Network debug/visualization tool** — genuinely useful standalone for anyone using Zenoh as a robotics bus; discovery via liveliness+manifest generalizes beyond this project.
6. **egui/Zenoh session bootstrap** — shared plumbing between Achilles and the sim frontend (session setup, repaint-on-update glue) — worth factoring out once the second egui binary exists, not before.

Held off for now: telemetry/logging crate — wait until repeated patterns emerge.

---

## 9. Roadmap (Rust/Async Learning Path)

**Phase 0 — Rust fundamentals in isolation**
- Ownership/borrowing, traits, enums+pattern matching, error handling.
- Start the types crate: `Command` first, then grow as later phases need new types.
- Build the gamepad-trigger crate against `Command`.

**Phase 1 — Async fundamentals**
- `tokio` basics: tasks, channels, `select!`.
- Toy pub/sub loopback with `zenoh` alone.

**Phase 2 — First real node: Gamepad Adapter**
- Wire gamepad-trigger crate into a Zenoh publisher node. No RPC yet.

**Phase 3 — RPC pattern**
- Build the Zenoh RPC helper crate.
- Stand up the Constants node (one-shot `get`), call it from a throwaway client.

**Phase 4 — Parameter system**
- Define `Tunable` trait + `SetParam`/`GetParam`/`ListParams` in the types crate.
- Implement on one real node; wire a gamepad trigger to `set`; confirm `changed` fires.
- Add config.toml bounds fields alongside values.

**Phase 5 — Introspection + Achilles**
- Define `Introspectable` trait + liveliness token convention in the types crate.
- Build the egui Achilles: liveliness-based discovery, manifest-based graph, live param view/tweak via the Params Registry once it exists. This becomes your primary window into everything built from here on.
- Stand up the Params Registry node.

**Phase 6 — State Machine + Safety**
- Build State Machine node: `Mode` enum, `state/mode` pub + `mode/get` queryable, `safety/report` sub with severity policy, `Halt` handling.
- Build Safety Monitor as a minimal stub (even before real scan data exists) to validate the fault-report path end to end.
- Wire Gamepad's hard EStop trigger as the direct/unconditional path.

**Phase 7 — Sim Physics Engine + Driver Nodes**
- Build the Sim Physics Engine: physics tick, internal `sim/*` topics, subscribes motor commands.
- Build VESC/RPLidar/IMU driver nodes against the sim backend first — canonical topics identical to what real hardware will eventually produce.
- Verify Achilles graph reflects all of this correctly.

**Phase 8 — Planning node (the hard one)**
- Odometry first (fuses VESC + IMU → pose) — needed by everything after this.
- Pure Pursuit Controller, subscribing to LiDAR + `state/mode`, publishing Twist. Gains `Tunable` from the start, with `biased;` mode/estop-first `select!` ordering.
- MPC via `clarabel` once Pure Pursuit is solid, using `spawn_blocking` + race-against-mode pattern for the solve.

**Phase 9 — SLAM, Localization, Global Planner**
- SLAM against sim LiDAR + odom, publishing map + pose.
- Localization against a saved map.
- Global Planner turning a map/route into waypoints for Controller.

**Phase 10 — Embedded (real hardware only, once repaired)**
- embassy peripheral node + host-side serial bridge. Lowest priority.

**Phase 11 — Sim Frontend**
- egui visual front-end for the simulation, reusing Achilles's Zenoh/egui bootstrap plumbing where it makes sense, as a separate binary.

**Phase 12 — Integration & hardening**
- Full loop in sim, then on real hardware once repaired. Tune gains live via gamepad + Achilles. Add logging/telemetry patterns that became obviously necessary during Phases 5–11.

---

## 10. Notes for Future Conversations (Context for Claude)

- This project is Rust-only by philosophy except where a vendor SDK makes Python pragmatic (currently: OAK-D bridge, deferred). Don't suggest C++ alternatives.
- No LLM/VLA work belongs in this project.
- Zenoh queryables are the RPC mechanism — don't propose building a separate RPC layer.
- `candle` and `opencv` (Rust bindings) were deliberately dropped in favor of `ort` + `kornia-rs`/`image`/`imgproc`.
- OAK-D is currently deferred entirely — no camera use case yet; don't assume it's in scope unless the person reintroduces it.
- SE(2)/scan-matching is intentionally *not* a crate — plain code inside the node that needs it.
- Params are decentralized by design, validated via bounds declared in each node's own `config.toml` section — this was reconsidered and deliberately reaffirmed, don't propose re-centralizing the write path. The Params Registry is a read-only cache and config-loading dispatcher only, never authoritative.
- `state/mode` has exactly one writer: State Machine. No other node — including Achilles — ever publishes to it directly; mode changes go through the direct EStop path or the `safety/report` fault channel.
- `Halt` is a terminal shutdown mode, not an operating mode — don't conflate it with EStop/SWStop.
- The Sim Physics Engine is a backend behind the real driver node interfaces, not a replacement for them — VESC/RPLidar/IMU driver nodes' canonical outputs are identical in sim and on real hardware; only their ingestion source differs.
- Achilles is a separate binary from the Sim Frontend, built first, and its discovery mechanism is liveliness tokens + an `Introspectable` manifest queryable — never raw message snooping.
- `NODE_GUIDE.md` has two sanctioned exceptions to the single-`select!`-loop model: `spawn_blocking`-offloaded long solves (raced against priority branches), and aggregator/tool nodes (background tasks feeding shared state, no ordering guarantee needed). Don't flag these as violations — see the updated guide.
- The types/messages crate (`f1tenth-types`) is the single source of truth for cross-node types, including the newly added `Mode` enum, fault-report types, and `Introspectable` trait.
