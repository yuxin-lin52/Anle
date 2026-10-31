# 06 Functions

Status: implemented.

## Two body forms

The default is the formula body:

- Formula body: `add fn "name"(params) ret = expression` — a single expression, calls allowed, statements forbidden.
- Statement body: `add fn "name"(params) ret:` opens the body; a lone `;` closes it. Inside: local `add val`, re-binding, `if`/`while`/`for`/`match`, calls to other fns, recursion. The return value is `give expression`, and it must be the last statement (single exit; `give` inside a branch is an error).
- Parameters are read as `call val "name"` — statement bodies have no bare-identifier exemption. Parameter types and return types are mandatory.

## Calls

`call fn "name"(args)` — same block; cross-block and cross-module forms add prefixes (see [02](02-basics.md)). Calls to external C functions are declared with `add outs fn "name"(params) ret` and invoked only inside `unsafe`.

## First-class functions

Function types exist: `f fn(int) int`. Functions can be passed as parameters, stored in vals, placed in containers, returned, and compared for identity — pure function pointers, no capture.

## Why

- Formula bodies (the function equals its value, no `return`): a single-expression body is a validated beginner-friendly shape, and unambiguous for tooling.
- Statement bodies exist because library functions could not be written: `split` needs "loop and build the result", which a formula body cannot express (no loop, no statements) and a block cannot return.
- `give`: the meaning is "hand the value to the caller", in the same family as `call` / `take` / `kill` — short verbs, one meaning each.
- Single exit first: multiple exits need control-flow merge analysis. The single-exit form already covers "loop and build"; mid-body `give` waits for a real requirement.
- No closures: capture extends an outer value's lifetime, which collides with deterministic reclamation. Function records (a struct holding fns and data) cover the use cases with explicit lifetimes.
- Blocks never gain return values: block = process, fn = formula is a design-level decision. To get a value out of a block, write a fn at the end and fetch it explicitly.

## Not provided

- Block return values (the role boundary)
- Mid-body `give` / early return (control-flow merge analysis; a later batch)
- Closure capture and anonymous fns (see above)
- Runtime stack-overflow guards beyond the guard page (deep recursion is user responsibility)
