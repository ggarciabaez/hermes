# Sim Plan — Phase 7 rollout

Checklist for the Phase 7 rollout, in the order agreed on: backend sim first,
then a Rerun node for visualization, then the passthrough drivers. This doc is
the working plan; the canonical phase tracking lives in `roadmap.md`.

Per `f1tenth-rust-plan.md`: every node's interface is designed against real
hardware, but sensor/actuator backends run through the Sim Physics Engine until
the robot is repaired. Driver nodes' canonical output topics are identical
between sim and real — only the ingestion source differs.

## Order

1. hermes-types additions
2. Sim Physics Engine (simple room first)
3. Starter controller (gamepad/drive → twist)
4. Rerun node (visualizer)
5. Map parser (binary image + metadata) — evolves the sim world, defines the
   map format SLAM/Localization consume later
6. Passthrough drivers (VESC / RPLidar / IMU)
7. Docs / launch housekeeping

## 1. hermes-types additions

Before any node uses a new type (per `crates/hermes-types/DOCS.md` contract):

- New `NodeName` variants + `name()` + `get_priority()` arms
  (`crates/hermes-types/src/lib.rs:31-40`):
  - `Sim` → `"Sim"` → priority 2 (domain `robot/sim/`)
  - `VESC` → `"VESC"` → priority 3 (domain `robot/vesc/`)
  - `RPLidar` → `"RPLidar"` → priority 4 (domain `robot/rplidar/`)
  - `IMU` → `"IMU"` → priority 5 (domain `robot/imu/`)
  - `Controller` already exists (priority 2).
- New wire types (new module, e.g. `sensors.rs`, all postcard-encodable):
  - `Twist { v: f32, y: f32 }` — mirrors `GPadDrive` so the starter controller
    is a literal remap.
  - `LaserScan` — angle_min, angle_max, angle_increment, range_min, range_max,
    `ranges: Vec<f32>`.
  - `ImuData` — accel/gyro triples, minimal.
  - `VescTelemetry` — speed, steer, timestamp (fields are a design choice).
  - `Pose { x, y, theta }` — sim-internal ground truth for the visualizer.
  - `SimMap` / occupancy-grid type for the map parser (step 5). This is the
    format SLAM/Localization read/write in Phase 9 — define it once here.
- Update `crates/hermes-types/DOCS.md` and the "not yet built" list in
  `docs/hermes-types.md`.

## 2. Sim Physics Engine (`nodes/sim_physics`)

Follow the `nodes/DOCS.md` sequence exactly: single session, `Node::new`,
endpoints, seed mode via `get_mode`, `activate_liveliness` last, `biased;`
mode-first `select!` loop, `Halt` breaks, `ctrl_c` branch, `session.close()`.

- Pull `wheelbase` / `max_steer` via `fetch_constants()` (already in
  `nodes/params/Config.toml`).
- Own fixed-dt tick (e.g. 10 ms), independent of query rate. Kinematic bicycle
  model: `Twist { v, y }` → integrate pose. Simple first-order motor response
  (speed ramps toward commanded).
- Subscribe `robot/controller/twist` (motor cmd).
- Publish internal sim topics (never canonical sensor topics directly):
  - `robot/sim/pose` → `Pose`
  - `robot/sim/scan` → `LaserScan`
  - `robot/sim/imu` → `ImuData`
  - `robot/sim/motor_state` → `VescTelemetry`
- Simple room world for scan raycasts (walls from a hardcoded box) — replaced by
  the map parser in step 5.

## 3. Starter controller (beginning of `nodes/controller`)

- Reads `robot/gamepad/drive` (`GPadDrive`), remaps → `robot/controller/twist`
  (`Twist`). Manual-only, no Pure Pursuit yet.
- Seed of the Phase 8 Controller node. Include the stop action on exit
  (publish zero twist on `Halt` / `ctrl_c`) per the node guide.

## 4. Rerun node (`nodes/rerun`)

- Dep: `rerun` crate (~0.36). Viewer binary needed separately
  (`cargo install rerun-cli` or `pip install rerun-sdk`);
  `RecordingStreamBuilder::new(app).spawn()` launches it.
- Aggregator node (Exception 2): background task per subscription logging to
  the recording stream — `robot/sim/pose` (car pose + scan rays),
  `robot/sim/scan`, `robot/sim/motor_state`, and the map grid once the parser
  exists. Visualization tool only — no introspection/graph logic (that's
  Achilles).

## 5. Map parser (evolves step 2's world)

- Load `map_path` (already a constant in `nodes/params/Config.toml`).
- Recommend PGM + sidecar metadata (resolution, origin, occupancy) — the classic
  F1TENTH format, parseable with std, no `image` crate.
- Sim engine raycasts scans against the occupancy grid; rerun node renders the
  grid. The same `SimMap` type is what SLAM/Localization read/write in Phase 9.

## 6. Passthrough drivers

- `nodes/vesc-driver`: sim backend (default) subscribes `robot/sim/motor_state`
  → republishes `robot/vesc/telemetry`. Real backend (serial+JSON to VESC)
  behind a flag, stubbed.
- `nodes/rplidar-driver`: sim subscribes `robot/sim/scan` →
  `robot/rplidar/scan`. Real backend (RPLidar SDK) behind a flag, stubbed.
- `nodes/imu-driver`: sim subscribes `robot/sim/imu` → `robot/imu/imu`. Real
  backend (IMU peripheral) behind a flag, stubbed.
- Canonical topics identical sim/real — that is the point of the split. Each
  follows the node guide + own `DOCS.md`.

## 7. Docs / launch housekeeping

- Add new key rows to the map in `docs/DOCS.md` (controller/twist, sim/*,
  canonical driver topics); update the workspace tree + Status table.
- Add nodes to workspace `members` and `LAUNCH.toml`.
- Per-node `DOCS.md`; tick Phase 7 in `docs/roadmap.md`.
- Suggested: make `docs/roadmap.md` canonical and retire the root
  `ROADMAP.md` (divergence flagged in `docs/DOCS.md`).

## Key expression additions

| Key | Kind | Owner | Payload |
|---|---|---|---|
| `robot/controller/twist` | pub | Controller | `Twist` |
| `robot/sim/pose` | pub | Sim | `Pose` |
| `robot/sim/scan` | pub | Sim | `LaserScan` |
| `robot/sim/imu` | pub | Sim | `ImuData` |
| `robot/sim/motor_state` | pub | Sim | `VescTelemetry` |
| `robot/vesc/telemetry` | pub | VESC Driver | `VescTelemetry` |
| `robot/rplidar/scan` | pub | RPLidar Driver | `LaserScan` |
| `robot/imu/imu` | pub | IMU Driver | `ImuData` |

## Verification

`python orchestrator.py LAUNCH.toml` + achilles + rerun: gamepad drive →
controller → twist → sim ticks → rerun shows car + scan; Achilles graph shows
Sim → 3 drivers → canonical topics (the roadmap gate check).