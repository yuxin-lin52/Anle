# 14 Inline assembly

Status: implemented.

## How it works

Two forms, both `unsafe` territory (inline assembly is the fourth unsafe use, alongside MMIO, C interoperation, and `kill`):

- **Block-level asm**: `#: name b asm` opens a block whose body is raw assembly text. Compiled as a naked function — no prologue, no epilogue. For interrupt/exception entry points, context switches, and lock/atomic sequences. No parameters, no return value; the ABI convention is the caller's.
- **Inline asm**: `asm("template" : outputs : inputs : clobbers)` inside a fn, in GCC three-section form — one-to-one with clang's `__asm__("in al, dx" : "=a"(result) : "d"(port) : "memory")`. Outputs are `register -> 名`; read-write uses `<->`; inputs are `register <- expression`; clobbers list `memory` or register names by hand. Named operands are emitted as `%[名]`. A fn whose body ends in inline asm with an output constraint is an expression and may initialize a val.

Rules: the quarantine (the asm parentheses) is parsed by asm's own grammar, not Anle's — constraint words like `in`/`out` do not collide with Anle keywords. The quarantine is ASCII-only: templates, binding names, and clobber lists never contain Chinese. Templates do not cross architectures (`al`/`dx` are x86); gating by `only(x86_64)` / `only(arm64)` is the way to write per-architecture code.

## Why

- Bare metal needs it: kernel entry points, context switches, privileged registers (`CR0`/`CR3`/`LGDT`), and true atomics can only be written as inline assembly (true in C as well).
- Unconstrained inline asm was rejected because it would break the memory model — it is opaque to alias analysis, reclamation points, and `kill` tracking. Constrained declarations (declare inputs/outputs/clobbers/memory effects, and the compiler conservatively guards) do not break it. The cost is local: optimization and reclamation are conservative at the asm point.
- Constraints must be explicit: omitting side effects is an error — the discipline is loud-over-quiet.
- GCC three-section form over keyword-style constraints: clobbers are hand-listed, matching how asm programmers already write it.

## Not provided

- asm with runtime-built templates — constraints must be compile-time-known; the alternative is equivalent to unconstrained asm, never
- Cross-architecture portable asm — instruction sets differ by nature; write per-architecture blocks gated by `only(...)`
- Implicit constraint inference — inference is guessing, and wrong guesses are dangerous, never
- Standalone assembly files (Go `.s` style) — would require defining an Anle assembly ABI; inline plus C interoperation covers the cases
- asm in compile-time evaluation — compile-time execution would run target-machine instructions; never
