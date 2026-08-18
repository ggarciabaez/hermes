# Hermes — F1TENTH Rust Rewrite: Project Plan

**Project name: Hermes.** **Debug/visualization tool: Achilles.** Zenoh topic
namespace: `robot/...`.

**Goals:** learn Rust/async through a real project; replace a monolithic
GIL-bound Python stack with a decoupled, Zenoh-based node architecture.

**Status:** the physical robot is currently non-functional (snapped
suspension, Jetson Orin Nano not powering on). The project targets a
simulation-first workflow: every node's interface (topics, RPC shapes) is
designed against real hardware, but sensor/actuator backends run through a
Sim Physics Engine node until hardware is repaired. Driver nodes' canonical
output topics are identical between sim and real — only the ingestion source
differs.

**Working today:** Gamepad (mode/EStop requests, controller state publish)
and State Machine (mode gating and publish) have real implementations.
Everything else in this doc is design, not yet built.

---

## 1. Architecture Overview

**Bus:** Zenoh is the only inter-node transport, no exceptions. Two wire
patterns, chosen per use case:
- **pub/sub** — streaming/continuous data: commands, mode, sensor frames,
  Twist, fault reports.
- **queryable/get (RPC)** — request/reply shaped: mode changes, param
  get/set/list, manifests, map snapshots, solves.

**Priority:** the gamepad's mode/EStop requests run on high Zenoh priority
with `Drop` congestion control. Achilles' subscriptions run background
priority / best-effort so broad listening never contends with control
traffic.

**Language boundary:** every node is Rust unless a vendor SDK makes Python
pragmatic for that specific node. No such node is currently in scope.

**Discovery:** every node declares a Zenoh liveliness token at startup
(`robot/liveliness/<node_name>`), retracted on clean shutdown or session
death — this is the crash/alive-detection mechanism for the whole system.
Liveliness answers "is this node up," not "what's its current value" — those
are separate questions and are solved separately (see Mode's `get` queryable
below, and Params' per-node `list` queryable).

**Runtime:** `tokio` powers every node. Each node's `select!` loop polls
Zenoh, sensors, and does its own work independently — see `NODE_GUIDE.md`
for the shape every node follows.

**RPC calling convention:** two generic helper functions, `query()` and
`query_with()`, wrap the get/receive/decode dance so a call site is one line
instead of hand-rolling `session.get(...).recv_async()...` each time. Full
listing and rationale in `NODE_GUIDE.md`. This is deliberately *not* a trait
that binds a request type to a response type and a key-expr constant — with
only a couple of RPCs existing so far, that abstraction doesn't have a clear
payoff yet. It's parked as an open roadmap question (Section 6, Phase 3/4) to
revisit once enough RPCs exist to see whether they actually share a shape
worth abstracting.

**Node identity:** the `NodeName` enum (one variant per node, also used to tag
RPC callers) has a `name()` method returning a plain debug string. Both
derived conventions are now settled and baked into code: the liveliness topic
is `robot/<name>/liveliness`, and the params-config key / tunable-endpoint
stem is the same lowercase name (`[params.<name>]`, `robot/<name>/params/*`).

**State Machine's RPC gate is State-Machine-specific**, not a general
pattern other RPCs are expected to copy. It's described fully in Section 3.
Other RPCs are free to do whatever fits, subject only to the general node
rules in `NODE_GUIDE.md` (single `select!` loop, no ambient concurrency,
explicit timeouts on outgoing calls).

**A Zenoh-router storage/memory plugin** (available on `zenohd`, persists
last-published values and can answer `get`s against them automatically) could
in principle replace some hand-rolled `get` queryables and part of what the
Params node does. This is noted as a possible future option, not something
committed to — it adds a router-configuration dependency beyond just running
`zenohd` with defaults, so it's being left out of scope until there's a
concrete reason to take on that dependency.

---

## 2. Node List

| Node | Language | Pattern | Responsibility |
|---|---|---|---|
| **Gamepad** | Rust | pub (drive values, controller state) + RPC client (mode/EStop requests) | Owns the physical controller. Button→intent mapping is a plain internal enum, never serialized — nothing about button layout crosses Zenoh. Mode and EStop requests go out as an RPC call to State Machine. EStop specifically requires a two-button chord (two distinct buttons held simultaneously) so it can't be triggered by an accidental single press. Raw drive values (velocity/steering) get their own pub topic, read directly by Controller when in `Manual` mode — not modeled as an RPC and not wrapped in any mode-related type. |
| **State Machine** | Rust | pub (mode, on change) + queryable (mode get) + queryable (mode set) | Sole authoritative writer of `Mode`. Holds the current mode, serves a no-payload `get` queryable returning it, and serves a `set` queryable gated by the rules in Section 3. Does not know or care *why* a mode change was requested beyond who asked — no fault-report subscription, no policy logic beyond validating the transition itself. |
| **Lifeguard** | Rust | sub (scan/pose) + sub (fault reports) + RPC client (mode set) | A watchdog with three jobs: detect hazards directly from sensor data, evaluate fault reports coming in from any other node, and log every fault centrally (self-detected and externally-reported alike go through the same evaluate/log logic — self-detected ones just skip the publish step, since Lifeguard would otherwise be both publisher and subscriber of its own channel). When it decides a mode change is warranted, it calls State Machine's `set` RPC — the same call any other caller uses, no privileged bypass. |
| **Controller** | Rust | sub (LiDAR, mode, gamepad drive topic) + pub (Twist) | The only node that actually commands the car. Runs Pure Pursuit (later MPC) to produce Twist in `Auto`; in `Manual`, converts the gamepad's raw drive topic into the same Twist output, so manual input passes through the same bounds/limits as autonomous output regardless of source. Has no mode-setting authority of its own. |
| **Odometry** | Rust | sub (VESC telemetry, IMU) + pub (odometry) | Fuses wheel/VESC speed + steering + IMU into a pose estimate. |
| **SLAM** | Rust | sub (scan, odom) | Runs independently in `Manual` mode only — no dedicated mode of its own. Saves the resulting map to disk when done; nothing subscribes to it live during that run. A later, separate run picks up the saved map by pointing Params' config at it — SLAM has no runtime write path into any other node's live state. |
| **Localization** | Rust | sub (scan, odom, map) + pub (pose) | Active in both `Manual` and `Auto`, assuming a prior SLAM run produced a map that's now referenced in config. |
| **Global Planner** | Rust | pub/queryable (waypoints) | Holds the current target line — a racing line or other user-defined target. Precomputed lines are read from config rather than computed live. |
| **VESC Driver** | Rust | sub (motor cmd) + pub (telemetry) | Real: serial+JSON to VESC. Sim: subscribes to Sim Physics Engine's internal motor-state topic. Canonical output topic identical either way. |
| **RPLidar Driver** | Rust | pub (scan) | Real: RPLidar SDK. Sim: subscribes to Sim Physics Engine's internal scan topic. |
| **IMU Driver** | Rust | pub (imu) | Real: IMU peripheral. Sim: subscribes to Sim Physics Engine's internal imu topic. |
| **Sim Physics Engine** | Rust | sub (motor cmd) + pub (internal sim topics) | Runs its own physics tick independent of query rate, simulating motor/steering response and producing synthetic scan/imu/telemetry consumed by the three driver nodes above. Not on canonical sensor topics directly. |
| **Params** | Rust | one-shot serve (fixed values) + RPC server (tunables) | Parses a single config file once at boot. One section of that file holds fixed, never-changing values (wheelbase, max steering, calibration) served one-shot to whichever node asks. Another section holds each node's tunable starting values, dispatched to the owning node via the same `set` mechanism a live tweak would use — so boot-time config loading and a live parameter change are the same code path, not two. There is no separate mirror/registry node — see Achilles below for how live values get queried centrally. |
| **Achilles** | Rust (egui) | sub (everything, best-effort) + RPC client | Standalone debug/visualization tool, separate binary, own Zenoh session. Builds its graph from liveliness tokens plus each node's manifest (what it publishes/subscribes/serves), never from raw message content. Queries live parameter values directly via a wildcard `get` across every node's own `list` queryable — Zenoh's native multi-reply `get` fans this out without needing a dedicated mirror node. Live param tweaks are restricted, client-side only, to when current mode is `Standby` — not enforced by the owning node, just gated in Achilles' own UI. |
| **Sim Frontend** | Rust (egui) | sub + pub | Visual sim front-end, separate binary, reuses Achilles' Zenoh/egui bootstrap plumbing where practical. |

Not currently in scope: an inference service, a host-side serial bridge, an
embedded-peripherals node, and an OAK-D camera bridge. These were considered
earlier and are dropped entirely for now rather than deferred with a
placeholder — they can be reintroduced later if a concrete need shows up.

---

## 3. Mode / State Machine

```
Mode = Standby | Manual | Auto | EStop | SWStop | Halt
```

- **`Standby`** is both the boot-default and a general pause state — used any
  time operation should stop but the system should stay ready, rather than
  idling in `Manual`. It's also the only mode in which Achilles is allowed to
  write live parameter values.
- **`EStop`** is a hard stop. The only legal transition out of it is to
  `Halt` — recovery requires a process restart, not a mode request.
- **`SWStop`** is functionally almost identical to `EStop` — the robot could
  technically continue safely in either case — but it exists specifically as
  a *human-intervention flag*: Lifeguard uses it when something serious
  happened (a near-collision, an anomalous sensor value) and judges that
  continuing without a person looking at it is unsafe, even though the
  situation isn't as severe as one warranting `EStop`. `SWStop` recovers only
  to `Standby`, and only via an explicit mode request — never automatically,
  and never from Lifeguard itself (see the lockout below).
- **`Halt`** is terminal — shuts the whole system down, no transitions out.

**Transition legality**, as currently implemented:

| From \ To | Standby | Manual | Auto | EStop | SWStop | Halt |
|---|---|---|---|---|---|---|
| Standby | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Manual | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Auto | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| EStop | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ |
| SWStop | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ |
| Halt | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

**Open question, deliberately unresolved:** whether `Auto → Halt` (and
`Manual → Halt`) should be allowed to skip `EStop` entirely, as the table
above currently permits. This stays open until the system is fully
integrated end to end — at that point it'll be decided based on whether
skipping the intermediate stop stage is actually safe in practice, not
decided speculatively now.

**State Machine's `set` RPC is gated by three checks, in this order, and
this gate is specific to State Machine — no other node's RPC is expected to
follow this shape:**

1. **Source allowlist** — only `Gamepad`, `Achilles`, and `Lifeguard` may
   call this RPC at all.
2. **Transition legality** — the requested `(current, new)` pair must appear
   in the table above.
3. **SWStop lockout** — if current mode is `SWStop`, a request from
   `Lifeguard` is rejected outright, regardless of what it's requesting.
   Recovery from `SWStop` is reserved for a human or Achilles.

Rejections reply with an error string (`reply_err`); a successful transition
replies with a plain acknowledgement and updates the published mode
immediately. Late joiners get the current mode via a no-payload `get`
queryable served from the same in-memory value the `select!` loop already
owns — no separate ticker or heartbeat publish, and no second code path that
could drift out of sync with the mutation logic.

---

## 4. Params

One node, not two. It owns a single config file at boot, holding:
- **Fixed values** (wheelbase, max steering, calibration, map paths after a
  SLAM run) — served one-shot, `get`-only, no live subscription, and no
  runtime write path from any other node. A finished SLAM run's map is picked
  up by a *later* run's config, never written into a running instance.
- **Tunable starting values**, one section per owning node — pulled by each
  node at boot through its own `Tunable::set`, the same method a live `set`
  RPC invokes. There is exactly one way any tunable parameter ever changes,
  whether that's boot loading, a gamepad trigger, or Achilles' live-tweak UI.

Boot sequencing is pull-based: a node waits for the Params liveliness token
(`robot/params/liveliness`), calls `robot/params/get` for its
`[params.<name>]` slice, and applies each key/value through its `Tunable::set`.
The wire shape for slices is `Vec<(String, ParamValue)>` (type-erased scalars)
— raw TOML can't cross the postcard wire. Each param-owning node serves
`robot/<name>/params/{list,set,get}` and publishes
`robot/<name>/params/changed` on a successful mutation. Gamepad's
`deadzone`/`v_min`/`v_max` are the proof of the mechanism end to end.

There is no separate registry/mirror node. Achilles queries live values
directly: a wildcard `get` across every node's own parameter-list queryable
(`robot/*/params/list`), using Zenoh's native multi-reply `get` to fan the
request out and collect every node's response in one call. Per-node detail
(what a given node currently publishes/subscribes/serves) comes from that
node's own manifest, not from a centralized cache.

---

## 5. Gamepad — Command Handling

Nothing crosses the wire as a shared "Command" type. Instead:

- Gamepad's button→intent mapping is a plain enum, internal to the Gamepad
  node only.
- Mode and EStop intents become an RPC call to State Machine's `set`
  queryable.
- Raw drive values (velocity, steering) get their own pub topic, consumed
  directly by Controller in `Manual` mode.
- Any future non-mode trigger becomes its own RPC to whichever node owns
  that function — no shared enum needed, since each RPC call is already
  point-to-point typed by its own request/response structs.

---

## 6. Roadmap

**Phase 3 — RPC pattern** — in progress. The `query()`/`query_with()` helper
functions (see `NODE_GUIDE.md`) factor out the get/receive/decode boilerplate
that's currently hand-rolled inline. **Open discussion, revisit once Params
(Phase 4) or Achilles (Phase 5) is underway:** whether a request/response/
key-expr-binding trait is worth building once more RPCs exist — currently
deferred since there's no repeated pattern yet to abstract.

**Phase 4 — Params** — done. Single node serving both fixed values and
tunables from one config file (Section 4). Tunable mechanism proven end to
end on Gamepad; further nodes adopt it by implementing `Tunable` and serving
their `params/{list,set,get}` endpoints.

**Phase 5 — Introspection + Achilles** — not started. Manifest/liveliness-
based graph discovery, wildcard `get`-based live parameter view (no
registry).

**Phase 6 — State Machine + Lifeguard** — in progress. State Machine's mode
gate (Section 3) works end to end, including the no-payload `get` queryable
and clean `Halt` shutdown. Lifeguard itself is not yet built — its
fault-evaluation logic and the two open mode-table questions (Auto→Halt,
and the SWStop lockout's scope) should be nailed down before it's built out.

**Phase 7 — Sim Physics Engine + Driver Nodes** — not started.

**Phase 8 — Controller** — not started. Manual pass-through (gamepad drive
topic → Twist) is part of this node's scope from the start.

**Phase 9 — SLAM, Localization, Global Planner** — not started. SLAM's
run-then-save-then-later-load model (Section 2) needs to be reflected in
whatever process updates config between runs.

**Phase 10 — Sim Frontend** — not started.

**Phase 11 — Integration & hardening** — not started. This is also where the
Auto→Halt question gets settled, once the full system exists to test against.

---

## 7. Notes for Future Conversations (Context for Claude)

- Topic namespace is `robot/...`.
- There is no shared "Command" type anywhere in the types crate. Per-node
  internal action enums (like Gamepad's button mapping) are expected and
  should stay local, never promoted to a cross-node type.
- State Machine's three-step RPC gate (allowlist → transition → SWStop
  lockout) is specific to State Machine. Don't generalize it as a pattern
  other RPCs are expected to follow — other RPCs just follow the general
  node rules in `NODE_GUIDE.md`.
- Late-joiner visibility for Mode is a no-payload `get` queryable reading the
  same in-memory value the node's `select!` loop owns — not a heartbeat
  publish, and not solved by liveliness tokens (liveliness only answers
  "is this node alive," not "what's its current value").
- `RpcCall`-style trait abstraction over RPCs is an open, explicitly
  unresolved question — don't silently adopt or reject it in future docs;
  it's parked pending more RPCs existing to compare against.
- Params and Constants are one node, not two, and there is no separate
  registry/mirror node — Achilles queries live values via wildcard `get`
  directly against each node's own parameter queryable.
- Tunable endpoints are `robot/<name>/params/{list,set,get}` plus a
  `robot/<name>/params/changed` publish; the Params node serves `[constants]`
  and per-node slices at `robot/params/get`. Params wire shape is
  `Vec<(String, ParamValue)>` — postcard cannot round-trip raw
  `toml::Table`/`toml::Value`, so raw TOML never crosses the wire.
- Boot loading is pull-based: each node waits on the Params liveliness token
  and applies its fetched `[params.<name>]` slice through its own
  `Tunable::set`, the same code path a live `set` RPC uses.
- SWStop is a human-intervention flag, not a lesser technical failure mode —
  functionally close to EStop, differing mainly in that it's meant to be
  recoverable by a human, and Lifeguard itself can never clear it.
- Two mode-table questions are intentionally open (Auto→Halt legality, and
  whether the SWStop lockout should ever admit a Lifeguard escalation
  exception) — surface them again rather than resolving them silently.
- Inference service, host-side Serial Bridge, embedded-peripherals node, and
  OAK-D Bridge are not in scope. Don't reintroduce them into the node list
  without the person raising it again.
- A Zenoh-router storage/memory plugin is a noted possible future
  replacement for some hand-rolled `get` queryables — not committed to, not
  in the current design.
