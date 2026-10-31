# 02 Basic concepts

Status: implemented.

## Blocks

All code lives in blocks. A block opens with `#: name b` (born) and closes with `#: name d` (death). Blocks are flat — they never nest. `in #:parent` is for grouping in display only and has no semantic effect.

At run time, blocks chain: every block nobody references is a chain root, and all chains run top-to-bottom in source order; referenced blocks run where they are called. Mutually-calling blocks with no root are rejected at compile time.

## Creation and re-binding

Names are created with `add`, exactly once: `add val "name" = value`, `add fn "name" (params) ret = body`, `add type "name" = definition`. Re-binding never uses `add` — it is `val "name" = new-value`. Assigning to an uncreated name is a compile error; there is no "first assignment creates the variable".

`add` without `=` is a pure declaration (the fn/val name plus its signature); a later line supplies the body — the complement form has the same syntax minus `add`.

## Full explicitness

Reads are `call val "name"`, calls are `call fn "name"(…)`. Bare identifiers are rejected — `fn` and `val` may share a name, so a bare identifier cannot say which one it means.

## The verb slots

`call` (reference — the freshest value), `take` (deep copy — the value as it is right now), `kill` (explicit early reclamation). All three state their target's source explicitly; bare cross-block access is rejected.

## Why

- Flat blocks: a block is the unit of process and the unit of memory. Nesting would blur the boundary both depend on.
- `add` once: repeated `add` turns "change the value" into "recreate it". First-assignment-creates hides typos as new variables. Closing both ends makes misspellings loud errors.
- Full explicitness: `fn` and `val` may share a name, so reads must carry a kind tag. The cost is a few characters per read; the benefit is that no read ever depends on context.
