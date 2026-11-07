# 13 Portability and targets

Status: designed, not yet implemented.

## How it works

- A target is three orthogonal dimensions plus a stack budget: `system` (windows / linux / none-for-bare-metal) × `arch` (x86_64 / arm64) × `backend` (c / llvm / asm last), and `stack` (e.g. `8M`).
- **Capability facets.** Builtins are grouped into eight facets — `pure` (pure computation, zero dependencies, available even on bare metal), `console`, `file`, `process`, `time`, `random`, `hw`, `ffi`. Each target declares which facets it implements. A program using a missing facet is rejected at compile time with the missing list. Porting is paying per facet: the pure facet is free, and the minimal runnable target needs two functions (exit + output).
- Builtin preset targets (e.g. `windows/x86_64/c`, all eight facets, verified) plus a custom-target description file for everything else — the core field is `faces`. Adding a target is adding data, not changing the compiler.
- Block-level gating `only(目标)`: `#: greeting_win b only(windows)` — whole blocks in or out; unmatched blocks are not checked and produce no code. Main code stays branch-free. Semantic differences are absorbed by the base layer (the platform branch is written once, inside the compiler).
- Stack: the budget can be set in the build config (written into the artifact at build time); guard pages plus runtime glue always backstop it — stack exhaustion is a clean "stack exhausted" error with exit 1, never a raw segfault.
- Build configuration: a minimal key-value format, two files (`lib.anle` package metadata, `build.anle` build config), UTF-8 without BOM, LF.

## Why

- The essence of porting: the compiler never needs to know how a new system does anything — it only emits symbol calls, and the target provides what those symbols mean.
- A custom target is data, not code: adding a target is writing a file.
- Capability facets: source code stays branch-free at statement level — platform differences are absorbed inside the base layer (Chinese file paths, platform sleep APIs — all inside the compiler). Block-level gating handles the differences that cannot be absorbed, in declaration form: whole blocks in or out, excluded blocks as if they never existed.
- No probe-and-degrade ("facet missing? switch to another spelling"): that violates the binary principle. The correct posture is source written inside the common capability subset.
- Statement-level conditional compilation (`#if`) is not adopted for the same reason pure indentation was rejected: it makes meaning depend on symbol placement. Block-level gating plus compile-time branch pruning covers every case.

## Not provided

- Statement-level conditional compilation (`#if`) — block gating plus compile-time branches cover the cases, never
- Probe-and-degrade — violates the binary principle, never
- Runtime stack configuration — fixed at build time, never
- Variable-name-level debug mapping — deep debugging uses the print convention
- Thread stack policy — decided during the concurrency implementation
