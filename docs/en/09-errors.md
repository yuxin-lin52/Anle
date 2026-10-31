# 09 Errors and error codes

Status: implemented.

## How it works

Failures are split in two, and the split is decided by types at design time:

| Category | Mechanism | Example |
|---|---|---|
| **Bug** (a state that should never exist) | **Guard termination**: division by zero, index out of bounds, dangling handle, safe-level overflow, runaway recursion → error with a line number, exit 1 | `10 / 0` |
| **Expected failure** (part of the normal flow) | **Error values**: sum types + match with a compile-time exhaustiveness check — errors are ordinary values | missing file, invalid input |

```anle
add type "Result" = Ok(int) | Err(str)
add fn "safe-div" (a int, b int) Result = (b == 0) ? Err("divide by zero") : Ok(a / b)
match call fn "safe-div"(10, 2):
case Ok("v"):
;1 print(call val "v")
;
case Err("reason"):
;1 print(call val "reason")
;
;
```

- Propagation has no sugar: call, unpack, continue. The verbose parts can be factored into fns.
- **Error codes**: compile-time diagnostics use `E0–E4` by stage (lexical / syntax / check / IR / backend) and `E5–E9` by ownership (project config / environment / compiler internals / reserved / fallback). The single rule: **if fixing your own source can remove the error, it is E0–E4; otherwise E5–E9**. Runtime guards use **R codes** (R01 dangling handle, R02 division by zero, R03 overflow, R04 out of bounds, R05 missing key, R06 out of memory).
- Diagnostics carry four parts: location (file, line, column), the action, the error, and **how to fix it**. Severity has four levels aligned with LSP.

## Why

- The two tracks are the necessary shape: letting a bug continue is meaningless, and an expected failure is part of the flow. The difference between languages is only whether the bug track is deterministic. Anle makes it deterministic: an error with a line number, not undefined behavior.
- Errors as ordinary values: zero hidden control flow and zero runtime overhead. Plain return codes lack type support, allow silent ignoring, and need endless boilerplate — sum types with compile-time exhaustive matching close all three.
- No propagation sugar: sugar depends on the language's expression structure. With fn bodies and match opened up, the verbosity is manageable via helper fns. If a real project proves the pain exceeds the threshold, sugar is evaluated on its own.
- Exceptions and try/catch never: hidden control flow plus a runtime unwinder, conflicting with AOT, no-runtime, and the binary-decision principle.
- The error-code split rule: "can fixing your own source remove it" cuts the two axes cleanly and prevents stage/ownership overlap.

## Not provided

- Propagation sugar (`?`-style) — re-evaluated if a real project proves the pain
- Error stacks / backtraces — debugging relies on reported line numbers
- Exceptions, try/catch — never
- Migrating existing builtins to error values — large break, zero gain; new code uses the error-value style
