# Shared Crates

Library crates every node may depend on. Two exist:

| Crate | Purpose |
|---|---|
| `hermes-types` | Every cross-node type, plus the `Node` helper that wraps Zenoh |
| `hermes-macros` | Declarative macros used while writing a node |

## What belongs here

**`hermes-types` owns every type that crosses Zenoh, without exception.** A type
serialized by one process and deserialized by another must have exactly one
definition; postcard is not self-describing, so two structurally-identical
definitions that later drift produce a silent decode failure at runtime rather
than a compile error. If a node needs a new message shape, it goes in
`hermes-types` first and is used second.

**Anything internal to one node stays in that node.** Gamepad's button→intent
`Command` enum is the reference case: it never crosses the wire, so promoting it
to a shared crate would only couple nodes to one node's hardware layout. This is
a deliberate standing decision — see `docs/f1tenth-rust-plan.md` §5.

**The crate grows on demand.** Types are added when the node that needs them is
actually built, not speculatively ahead of the roadmap. `TopicInfo<T>` in
`hermes-types/src/lib.rs:83` is what the alternative looks like: a stub added
early, unused, with no consumer to shape it.

## Adding a crate

```bash
cargo new --lib crates/<name>
```

Then add `"crates/<name>"` to the root `Cargo.toml` `members`, and prefer
`{ workspace = true }` for shared third-party dependencies so versions stay
aligned across the workspace.

Before adding one, check whether it is really a third crate rather than a module
in `hermes-types`: the current split is "wire types + Zenoh plumbing" vs.
"macros", and the RPC helper that was originally planned as its own crate ended
up as `hermes_types::rpc` because it was too small to justify the boundary.

## Gaps

- **No tests in either crate.** Both are pure enough to test cheaply —
  `transition()`, `Param::set` bounds, `ParamValue` round-trips,
  `params_from_toml`.
- **No published docs pass.** Doc comments exist on the params system and are
  absent almost everywhere else; `cargo doc` output is thin.
- **`hermes-types` mixes two concerns** — plain wire types (usable anywhere) and
  the Zenoh-bound `Node` runtime. A node that only wants the types still pulls
  in `zenoh`, `tokio`, `botpad`, and `toml`. Splitting types from runtime is
  worth considering if compile times or a no-std embedded target ever matter.
