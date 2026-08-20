# hermes-macros

One declarative macro that collapses a node's endpoint declarations into a
single readable block. Not a proc-macro crate — it is a plain `macro_rules!`
with `#[macro_export]`, so it has no dependencies and no build-script cost.

## Features

### `setup_endpoints!`

```rust
setup_endpoints!(
    node,
    pubs: [
        state_node = "state",
        drive_node = "drive",
    ],
    queries: [
        request_mode = "request_mode",
        get_mode     = "get",
    ]
);
```

Expands to one `let` per entry:

```rust
let state_node = node.declare_pub("state").await.unwrap();
let request_mode = node.declare_queryable("request_mode").await.unwrap();
```

The value is readability: the block is a legible table of everything the node
exposes, in the same shape across all three nodes, right where a reader looks
for it. Names are domain-relative — `"state"` becomes `robot/<node>/state` —
because `declare_pub`/`declare_queryable` prefix the node's domain.

## Guide

- **Context requirements**: expands to `.await`, so it must appear inside an
  `async fn`, with a `mut` binding named by the first argument that has the
  `Node` methods in scope.
- **Both sections are mandatory**, even when empty. Gamepad passes
  `queries: [ ]` (`nodes/gamepad-adapter/src/main.rs:86`) for exactly this
  reason.
- **Subscribers are not covered** — declare them separately with
  `node.sub_mode()` or `node.declare_sub(&full_key)`, which take full key
  expressions rather than domain-relative names.
- **It panics on failure.** `unwrap()` is baked into the expansion, which is
  acceptable during startup (a node that cannot declare its endpoints has
  nothing useful to do) but means it must never be used after the node is live.
- **Import it as a normal item**: `use hermes_macros::setup_endpoints;`.
  `#[macro_export]` puts it at the crate root regardless of the module it is
  defined in.

Adding a macro: define it in `src/lib.rs` with `#[macro_export]` and a doc
comment showing a usage example, following the existing one. Keep macros to
declaration-site boilerplate; anything with logic belongs in `hermes-types` as a
function, where it can be read and tested.

## Missing / known issues

- **No `subs:` section**, so a node's endpoint declarations are split across the
  macro and loose `declare_sub` calls. Adding one is awkward because subs take
  full key expressions, not domain-relative names — the natural fix is to make
  `declare_sub` take `NodeName + endpoint` first
  (`crates/hermes-types/src/lib.rs:127`) and then add the section.
- **Sections cannot be omitted**, forcing `queries: [ ]` noise.
- **No error propagation variant** — a `try_setup_endpoints!` expanding to `?`
  would let a node that wants to degrade gracefully do so.
- **Not hygiene-checked against shadowing**: the macro introduces bindings by
  the names you give it, so a name collision with an existing local silently
  shadows it.
- **No tests** — not even a compile-pass test, so a breaking edit to the macro
  is only caught when a node fails to build.
- **Related dead macro**: `set_triggers!`
  (`nodes/gamepad-adapter/src/utils.rs:15`) is a gamepad-local macro of the same
  flavour that is defined and never used. If it earns its place, it belongs
  here; otherwise it should be deleted.
