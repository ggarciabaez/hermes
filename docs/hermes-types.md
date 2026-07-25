# hermes-types — Crate Design Reference

Single source of truth for every cross-node type/trait in the project. Every node
depends on this crate; nothing hand-rolls its own message shapes. Treat this doc as
the design contract — update it *before* adding a type to the crate, not after, so it
never drifts out of sync with the code.

Per Phase 0 of the plan: don't pre-build all of this speculatively. Start with
`Command`, add the rest as each roadmap phase actually needs it. This doc describes
the target shape you're building toward, not a checklist to implement in one sitting.

## Quick reference

| Name | Kind | Used by | Purpose |
|---|---|---|---|
| `Mode` | enum | State Machine (writes), all control nodes (read) | Robot operating state |
| `Severity` | enum | any node (write), State Machine (read) | Fault report priority |
| `ParamValue` | enum | any `Tunable` node, Params Registry, debug tool | Type-erased wire value for param RPCs |
| `Command` | enum | Gamepad Adapter (pub), State Machine + Controller (sub) | Manual input |
| `Pose` | struct | Odometry, SLAM, Localization, Global Planner | SE(2) position + heading |
| `Twist` | struct | Controller (pub), VESC Driver (sub) | Commanded velocity/steering |
| `LaserScan` | struct | RPLidar Driver (pub), SLAM/Localization/Safety Monitor (sub) | LiDAR frame |
| `ImuData` | struct | IMU Driver (pub), Odometry (sub) | IMU frame |
| `VescTelemetry` | struct | VESC Driver (pub), Odometry (sub) | Motor/steering telemetry |
| `Waypoint` / `Path` | struct | Global Planner (pub), Controller (sub) | Reference trajectory |
| `OccupancyGrid` | struct | SLAM (pub/serve), Global Planner/Localization/Sim Frontend (sub) | Map |
| `FaultReport` | struct | any node (pub), State Machine (sub) | Fault channel payload |
| `Bounds<T>` | generic struct | any `Param<T>` | Declarative min/max, from `config.toml` |
| `Param<T>` | generic struct | every node with tunables | Value + validation, backing store for `Tunable` |
| `ParamInfo` | struct | `Tunable::list()`, Params Registry, debug tool | One param's wire-safe snapshot |
| `NodeManifest` / `TopicInfo` / `QueryableInfo` | struct | `Introspectable::manifest()`, debug tool | Self-description for graph building |
| `SetParamRequest/Response` etc. | struct | any RPC caller ↔ `Tunable` node | Param RPC wire types |
| `Tunable` | trait | every node with live params | `get`/`set`/`list` |
| `Introspectable` | trait | every node | `manifest()` |

---

## 1. Enums

### `Mode`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode { Manual, Auto, Slam, EStop, SwStop, Halt }
```
Published exclusively by State Machine on `f1tenth/state/mode`. Every control node
subscribes and treats it as a high-priority (`biased;`) `select!` branch. `Halt`
triggers the same shutdown path as `ctrl_c`.

### `Severity`
```rust
pub enum Severity { Info, Warning, Critical }
```
Carried in `FaultReport`. Any node can publish one; only State Machine consumes it
to decide policy (Critical→EStop, Warning→SwStop/forced Manual, Info→log only).

### `ParamValue`
```rust
pub enum ParamValue { F32(f32), F64(f64), I32(i32), Bool(bool), String(String) }
```
Type-erased wire value. Needed because a generic "list all params" caller (Params
Registry, debug tool) can't know each node's concrete param types ahead of time.
Nodes convert to/from this only at the RPC boundary — internal logic stays concrete
(`Param<f32>`, etc.).

### `Command`
```rust
pub enum Command {
    Drive { velocity: f32, steering: f32 },
    ModeRequest(Mode),
    EStop, // hard trigger — direct to State Machine, bypasses safety/report entirely
}
```
Published by Gamepad Adapter on `f1tenth/commands/manual`. `EStop` is handled as a
special case downstream — never routed through the fault-report/policy path.

---

## 2. Structs

### `Pose` / `Twist`
```rust
pub struct Pose(pub nalgebra::Isometry2<f32>);
pub struct Twist { pub linear: f32, pub angular: f32 }
```
`Pose` wraps nalgebra's SE(2) isometry — used wherever a position+heading estimate is
produced or consumed (Odometry, SLAM, Localization, Global Planner). `Twist` is
Controller's output on `f1tenth/cmd/twist`, consumed by VESC Driver.

### `LaserScan`
```rust
pub struct LaserScan {
    pub angle_min: f32,
    pub angle_max: f32,
    pub angle_increment: f32,
    pub ranges: Vec<f32>,
    pub timestamp: u64,
}
```
Published by RPLidar Driver (real or sim-backed — canonical shape either way) on
`f1tenth/sensors/scan`. Consumed by SLAM, Localization, Safety Monitor.

### `ImuData`
```rust
pub struct ImuData {
    pub angular_velocity: [f32; 3],
    pub linear_acceleration: [f32; 3],
    pub timestamp: u64,
}
```
Published by IMU Driver on `f1tenth/sensors/imu`. Consumed by Odometry.

### `VescTelemetry`
```rust
pub struct VescTelemetry {
    pub rpm: f32,
    pub current: f32,
    pub voltage: f32,
    pub steering_angle: f32,
    pub timestamp: u64,
}
```
Published by VESC Driver on `f1tenth/sensors/vesc_telemetry`. Consumed by Odometry.

### `Waypoint` / `Path`
```rust
pub struct Waypoint { pub pose: Pose, pub velocity: f32 }
pub struct Path(pub Vec<Waypoint>);
```
Published/served by Global Planner on `f1tenth/plan/waypoints`, consumed by Controller.

### `OccupancyGrid`
```rust
pub struct OccupancyGrid {
    pub width: u32,
    pub height: u32,
    pub resolution: f32,
    pub origin: Pose,
    pub data: Vec<i8>, // -1 unknown, 0-100 occupancy probability
}
```
Published + served (`map/get`) by SLAM. Consumed by Global Planner, Localization,
Sim Frontend / debug tool for display.

### `FaultReport`
```rust
pub struct FaultReport {
    pub source: String,
    pub severity: Severity,
    pub message: String,
    pub timestamp: u64,
}
```
Published by any node to `f1tenth/safety/report`. Subscribed solely by State Machine.

### `Bounds<T>`
```rust
pub struct Bounds<T> { pub min: T, pub max: T }
```
Generic. Deserialized straight from a node's `config.toml` (`bounds = [0.1, 0.5]`).
Feeds `Param<T>`.

### `Param<T>` — the one real generic in this crate
```rust
pub struct Param<T> {
    value: T,
    bounds: Option<Bounds<T>>,
}

impl<T: PartialOrd + Copy> Param<T> {
    pub fn new(value: T, bounds: Option<Bounds<T>>) -> Self { Self { value, bounds } }
    pub fn get(&self) -> T { self.value }
    pub fn set(&mut self, new: T) -> Result<(), ParamError> {
        if let Some(b) = &self.bounds {
            if new < b.min || new > b.max {
                return Err(ParamError::OutOfBounds);
            }
        }
        self.value = new;
        Ok(())
    }
}
```
Every tunable field on every node is a `Param<T>` — pairs a value with optional
bounds and centralizes the reject-out-of-range logic so no node hand-rolls its own
check. A node's `Tunable` impl becomes mostly a `match` from string key to one of its
`Param<T>` fields, converting to/from `ParamValue` only at the RPC boundary.

### `ParamInfo`
```rust
pub struct ParamInfo {
    pub key: String,
    pub value: ParamValue,
    pub bounds: Option<(ParamValue, ParamValue)>,
}
```
Returned by `Tunable::list()`. Used by Params Registry to build its cache and by the
debug tool to render a tweakable UI.

### `NodeManifest` / `TopicInfo` / `QueryableInfo`
```rust
pub enum Direction { Publishes, Subscribes }

pub struct TopicInfo {
    pub key_expr: String,
    pub type_name: String,
    pub direction: Direction,
}

pub struct QueryableInfo {
    pub key_expr: String,
    pub request_type: String,
    pub response_type: String,
}

pub struct NodeManifest {
    pub node_name: String,
    pub topics: Vec<TopicInfo>,
    pub queryables: Vec<QueryableInfo>,
}
```
Returned by every node's `Introspectable::manifest()`. This is the *only* source the
debug tool uses to build its graph — never raw message payloads.

---

## 3. RPC request/response types

```rust
pub struct SetParamRequest { pub key: String, pub value: ParamValue, pub source: String }
pub struct SetParamResponse { pub result: Result<(), ParamError> }
pub struct GetParamRequest { pub key: String }
pub struct GetParamResponse { pub value: Option<ParamValue> }
pub struct ListParamsRequest; // empty
pub struct ListParamsResponse { pub params: Vec<ParamInfo> }

pub enum ParamError { NotFound, OutOfBounds, TypeMismatch }
pub enum RpcError { Timeout, Unavailable, Remote(String) }
```
`source` on `SetParamRequest` exists so logs can distinguish caller identity —
gamepad, debug tool, Params Registry boot-load — all through the identical RPC shape,
no privileged bypass for any of them.

---

## 4. Traits

### `Tunable`
```rust
pub trait Tunable {
    fn get(&self, key: &str) -> Option<ParamValue>;
    fn set(&mut self, key: &str, value: ParamValue) -> Result<(), ParamError>;
    fn list(&self) -> Vec<ParamInfo>;
}
```
Implemented by every node with live-adjustable params. Backing storage is one
`Param<T>` field per parameter; this trait is the thin dispatch layer serving
`f1tenth/params/<node>/{get,set,list}`.

### `Introspectable`
```rust
pub trait Introspectable {
    fn manifest(&self) -> NodeManifest;
}
```
Implemented by every node, serving `f1tenth/introspect/<node>`.

---

## 5. Topic / key-expression conventions

Not exhaustively enumerated as constants yet — add to a `topics` module in this
crate as each is actually implemented, so the module stays truthful:

- `f1tenth/state/mode`, `f1tenth/state/pose`, `f1tenth/state/odom`
- `f1tenth/safety/report`
- `f1tenth/commands/manual`
- `f1tenth/sensors/{scan,imu,vesc_telemetry}`
- `f1tenth/cmd/twist`
- `f1tenth/map/occupancy_grid`, `f1tenth/map/get` (queryable)
- `f1tenth/plan/waypoints`
- `f1tenth/params/<node>/{get,set,list,changed}` (queryables + pub)
- `f1tenth/introspect/<node>` (queryable)
- `f1tenth/liveliness/<node>` (liveliness token)
