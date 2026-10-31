# 08 Compile-time evaluation

Status: implemented. Container constants are not yet implemented.

## How it works

- **Constants are the fifth named kind**: `add fix "buf" int = 4096`. They cannot be re-assigned (`val "名" = …` on a fix is an error) — that is the semantics, not a restriction to be lifted.
- **Compile-time execution triggers automatically**: a call whose arguments are all known at compile time is interpreted at compile time, and the result is used as a value. There is no comptime annotation — compile-time-ness is a property of values, not another kind of code.
- **Compile-time branches need no new syntax**: an `if` with a compile-time-known condition is pruned; the untaken side produces no code.
- **Failure is loud**: a constant expression that cannot be evaluated is a compile error — never a silent fallback to runtime.
- Red line: **no I/O and no dependence on runtime values** at compile time.

```anle
add fix "buf" int = 4096
add fn "twice" (n int) int = n * 2
add fix "answer" int = call fn "twice"(21)
if 2 > 3:
;1 print("this never produces code")
;
```

## Why

- What is bought is capability; performance rides along. The real gains: table pre-computation (startup work drops to zero, the table lands in read-only memory), branch pruning (the untaken side emits nothing), and static layout (sizes known at compile time land in the data segment). Simple constant folding is something -O2 already does — not the selling point.
- Automatic triggering instead of annotations: compile-time-ness is a property of values. Requiring a comptime marker on every function makes the feature heavier than the idea.
- Immutable constants pair with mutable vals as a binary: a `fix` is decided at compile time; re-assignment is an error, not a fallback.
- The macro alternative: of the four uses of macros — boilerplate removal (generics), compile-time tables (this mechanism), domain dialects (rejected: conflicts with the philosophy), auto-derivation (reflection, far future). Text macros collide with the token-stream foundation. **Metaprogramming needs are served by compile-time evaluation plus generics; macros are not introduced.**

## Not provided

- Explicit comptime annotation syntax — never
- Compile-time I/O — red line, never
- Container constants — need static data-segment support; currently a loud error, never a silent fallback
- Statement-level interpretation (locals, loops at compile time) — expands with the language
- Compile-time reflection — a far-future topic (depends on type-introspection design)
