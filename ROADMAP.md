# Hermes — Roadmap & Checklist

Sim-first F1TENTH rewrite in Rust/Zenoh. Debug tool: **Achilles**. Types crate:
`hermes-types` (grows on-demand, per Phase 0 — don't pre-build).

Timeline assumes a part-time learning pace (~5–8 focused hrs/week). Adjust freely —
the *ordering* matters more than the calendar; each phase unlocks the next.

---

## Phase 0 — Rust fundamentals in isolation
**~1–2 weeks**
- [ ] Ownership/borrowing, lifetimes basics
- [ ] Traits, generics, enums + pattern matching
- [ ] Error handling (`Result`, `?`, custom error enums)
- [ ] Start `hermes-types` crate: implement `Command` enum only
    - [ ] `Command::Drive { velocity, steering }`
    - [ ] `Command::ModeRequest(Mode)` (stub `Mode` if needed, or defer)
    - [ ] `Command::EStop`
- [ ] Build gamepad-trigger crate skeleton against `Command` (no Zenoh yet)
- [ ] **Gate check:** can construct/match all `Command` variants confidently before moving on

## Phase 1 — Async fundamentals
**~1 week**
- [ ] `tokio` basics: `spawn`, channels (`mpsc`, `watch`), `select!`
- [ ] Toy pub/sub loopback using `zenoh` alone (no project types yet)
- [ ] Understand `biased;` ordering in `select!`
- [ ] **Gate check:** comfortable writing a `select!` loop with 2–3 branches from memory

## Phase 2 — First real node: Gamepad Adapter
**~1 week**
- [ ] Wire gamepad-trigger crate into a Zenoh publisher
- [ ] Publish `Command` on `f1tenth/commands/manual`
- [ ] No RPC yet — pure pub only
- [ ] Verify with a throwaway subscriber (or `zenoh sub` CLI)
- [ ] **Gate check:** node runs, publishes real gamepad input, no RPC/mode logic yet

## Phase 3 — RPC pattern
**~1 week**
- [ ] Build Zenoh RPC helper crate: `get`/queryable wrapper + `postcard` serialization
- [ ] Typed request/response generics, explicit timeout on every call
- [ ] Stand up **Constants** node (one-shot `get`, no live subscription)
- [ ] Call it from a throwaway client to confirm the RPC helper works end-to-end
- [ ] **Gate check:** RPC helper reusable without copy-pasting boilerplate per node

## Phase 4 — Parameter system
**~1–2 weeks**
- [ ] Add to `hermes-types`: `ParamValue`, `Bounds<T>`, `Param<T>`, `ParamInfo`
- [ ] Add `SetParamRequest/Response`, `GetParamRequest/Response`, `ListParamsRequest/Response`, `ParamError`
- [ ] Define `Tunable` trait (`get`/`set`/`list`)
- [ ] Implement `Tunable` on one real node (good candidate: Gamepad Adapter or a stub)
- [ ] Wire a gamepad trigger to fire a `SetParam` RPC
- [ ] Confirm `changed` topic fires on successful mutation
- [ ] Add `bounds = [min, max]` fields to that node's `config.toml`
- [ ] **Gate check:** out-of-bounds `set` rejected with `ParamError::OutOfBounds`, not silently clamped

## Phase 5 — Introspection + Achilles (debug tool)
**~2–3 weeks** (this is a real chunk of work — it's your primary tool from here on)
- [ ] Add to `hermes-types`: `Direction`, `TopicInfo`, `QueryableInfo`, `NodeManifest`
- [ ] Define `Introspectable` trait (`manifest()`)
- [ ] Define liveliness token convention: `f1tenth/liveliness/<node_name>`
- [ ] Implement `Introspectable` + liveliness on Gamepad Adapter + Constants (nodes so far)
- [ ] Build Achilles (egui, native binary, own Zenoh session)
    - [ ] Liveliness-based node discovery (crash detection via retraction)
    - [ ] Manifest-based graph view (topics/queryables, never raw payload snooping)
    - [ ] Background-task-per-topic ingestion (Exception 2 pattern — no `select!`)
    - [ ] Live stats via Zenoh admin/stats space
    - [ ] Param view/tweak UI → goes through real `SetParam` RPC, includes `source` field
- [ ] Stand up **Params Registry** node
    - [ ] Subscribes to every node's `changed` topic → read-only cache
    - [ ] `list`/`get-all` queryable for dashboards/Achilles
    - [ ] Boot-time config loader: reads `config.toml`, dispatches `SetParam` RPCs (same path as gamepad)
- [ ] **Gate check:** Achilles shows live graph of Gamepad Adapter + Constants + Params Registry correctly

## Phase 6 — State Machine + Safety
**~2 weeks**
- [ ] Add to `hermes-types`: `Mode` enum, `Severity` enum, `FaultReport` struct
- [ ] Build **State Machine** node
    - [ ] `state/mode` pub (sole authoritative writer)
    - [ ] `mode/get` queryable (late-joiner support)
    - [ ] `safety/report` sub with severity policy (Critical→EStop, Warning→SWStop/forced Manual, Info→log only)
    - [ ] `Halt` handling — treat identically to `ctrl_c` shutdown path
    - [ ] Central logging of every fault report regardless of outcome
- [ ] Build **Safety Monitor** as a minimal stub (even with no real scan data yet)
    - [ ] Publishes to `safety/report`, never writes `state/mode` directly
- [ ] Wire Gamepad's hard `EStop` trigger as the direct/unconditional path (bypasses fault-report policy entirely, highest Zenoh priority + `Drop` congestion control)
- [ ] **Gate check:** EStop from gamepad forces `Mode::EStop` with no perceptible delay; a fake `Warning` fault correctly triggers `SWStop`/forced `Manual`

## Phase 7 — Sim Physics Engine + Driver Nodes
**~2–3 weeks**
- [ ] Add to `hermes-types`: `LaserScan`, `ImuData`, `VescTelemetry`
- [ ] Build **Sim Physics Engine**
    - [ ] Own physics tick loop, independent of query rate
    - [ ] Subscribes motor commands
    - [ ] Publishes internal `sim/*` topics (scan, imu, telemetry) — not canonical topics directly
- [ ] Build **VESC Driver** (sim backend: subscribes to `sim/*` motor-state; canonical output topic identical to real)
- [ ] Build **RPLidar Driver** (sim backend: subscribes to `sim/*` scan)
- [ ] Build **IMU Driver** (sim backend: subscribes to `sim/*` imu)
- [ ] Confirm all three driver nodes implement `Introspectable` + liveliness
- [ ] **Gate check:** Achilles graph now shows Sim Engine → 3 drivers → canonical topics, correctly labeled

## Phase 8 — Planning node (the hard one)
**~3–4 weeks**
- [ ] Add to `hermes-types`: `Pose`, `Twist` (nalgebra `Isometry2` wrapper)
- [ ] Build **Odometry** node
    - [ ] Subscribes VESC telemetry + IMU
    - [ ] Fuses into pose estimate → publishes `state/odom`
- [ ] Build **Controller** node — Pure Pursuit first
    - [ ] Subscribes LiDAR + `state/mode` (biased, mode/estop-first `select!`)
    - [ ] Publishes `Twist` at ~50Hz
    - [ ] Gains are `Tunable` from the start (own `Param<T>` fields, `config.toml` bounds)
- [ ] Add MPC via `clarabel` once Pure Pursuit is solid
    - [ ] Solve runs via `spawn_blocking` (Exception 1 pattern)
    - [ ] `select!` races solve `JoinHandle` against `state/mode` branch, `biased;`
    - [ ] Stale/preempted solve result discarded, not force-cancelled
- [ ] **Gate check:** robot (sim) tracks a hardcoded path with Pure Pursuit; EStop mid-solve stops actuation immediately even during an MPC solve

## Phase 9 — SLAM, Localization, Global Planner
**~3–4 weeks**
- [ ] Add to `hermes-types`: `OccupancyGrid`, `Waypoint`, `Path`
- [ ] Build **SLAM** node
    - [ ] Subscribes sim LiDAR + odom
    - [ ] Publishes `map/occupancy_grid`
    - [ ] Serves `map/get` queryable for late-joiners (not via Constants)
    - [ ] Scan matching (ICP) as plain in-node code, not a separate crate
- [ ] Build **Localization** node
    - [ ] AMCL-style pose estimate against a saved static map
    - [ ] Active in `Auto` mode, subscribes scan + odom + map
    - [ ] Publishes `state/pose`
- [ ] Build **Global Planner** node
    - [ ] Turns map/route into `plan/waypoints` (pub + queryable)
    - [ ] Consumed by Controller
- [ ] **Gate check:** full sim loop — SLAM builds a map, Localization holds a pose against it, Global Planner produces waypoints, Controller tracks them

## Phase 10 — Embedded (real hardware only, once repaired)
**~2 weeks** — blocked until suspension/Jetson issue is fixed
- [ ] embassy-based peripheral node (LEDs/indicators, possibly backup IMU)
- [ ] Host-side Serial Bridge (postcard+COBS framing, republish onto Zenoh)
- [ ] Lowest priority in the system — do last, don't let it block sim-side progress
- [ ] **Gate check:** peripheral state visible and controllable via Zenoh/Achilles

## Phase 11 — Sim Frontend
**~1–2 weeks**
- [ ] egui visual front-end for the simulation, separate binary
- [ ] Reuse Achilles's Zenoh/egui bootstrap plumbing (extract shared session-setup + repaint-on-update glue if not already factored out)
- [ ] **Gate check:** can watch the sim robot move/localize/plan visually, not just via Achilles's graph view

## Phase 12 — Integration & hardening
**~ongoing, open-ended**
- [ ] Full loop validated in sim
- [ ] Full loop validated on real hardware (once Phase 10 unblocks)
- [ ] Tune gains live via gamepad + Achilles
- [ ] Add logging/telemetry patterns that became obviously necessary during Phases 5–11 (deferred crate — don't build speculatively)
- [ ] Revisit deferred items: OAK-D Bridge (if a camera use case appears), IMU-on-OAK-D open item

---

## Running architectural checklist (apply throughout, not phase-specific)

- [ ] Every node implements `Introspectable` + declares a liveliness token before it's considered "done"
- [ ] Every control node follows the single-`select!`-loop model unless it's Exception 1 (`spawn_blocking` + race) or Exception 2 (aggregator/tool node)
- [ ] Every control node treats `state/mode` as a `biased;`-first branch; `Halt` handled identically to `ctrl_c`
- [ ] No node infers mode from anything other than `state/mode`
- [ ] No node hand-rolls param validation outside `Param<T>`/`Tunable`
- [ ] All faults go through `f1tenth/safety/report` — no node-local fault logging
- [ ] `hermes-types` updated *before* a new type is used, per its own contract
- [ ] `NODE_GUIDE.md` and `hermes-types.md` topic table stay in sync with what's actually implemented (topics module grows only as topics are added)

---

## Estimated total timeline

| Phase | Duration | Cumulative |
|---|---|---|
| 0 — Rust fundamentals | 1–2 wk | 2 wk |
| 1 — Async fundamentals | 1 wk | 3 wk |
| 2 — Gamepad Adapter | 1 wk | 4 wk |
| 3 — RPC pattern | 1 wk | 5 wk |
| 4 — Parameter system | 1–2 wk | 7 wk |
| 5 — Introspection + Achilles | 2–3 wk | 10 wk |
| 6 — State Machine + Safety | 2 wk | 12 wk |
| 7 — Sim Engine + Drivers | 2–3 wk | 15 wk |
| 8 — Planning node | 3–4 wk | 19 wk |
| 9 — SLAM/Localization/Planner | 3–4 wk | 23 wk |
| 10 — Embedded* | 2 wk | — (parallel/blocked) |
| 11 — Sim Frontend | 1–2 wk | 25 wk |
| 12 — Integration | ongoing | — |

\* Phase 10 is hardware-gated and doesn't block Phases 8–9/11, which are all
sim-backed — it can slot in whenever the robot is repaired, in parallel with later
phases if convenient.

**Rough total: ~6 months** at 5–8 hrs/week to a working sim-only autonomous loop
(through Phase 9), plus Sim Frontend polish. Real-hardware integration (Phase 10 + 12)
adds time on top, gated by the physical repair.