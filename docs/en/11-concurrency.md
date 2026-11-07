# 11 Concurrency: static watermark ordering

Status: semantics reserved (the design is locked so other designs do not collide with it; the implementation is scheduled separately).

## How it works

1. Operations on shared data are explicitly annotated with an ordering stamp (E1, E2, E3…). En waits for the watermark of E(n-1), then stamps its own when done. Parallel parts run in parallel; only the touch points of shared data wait in order.
2. Only smaller stamps may be waited on — deadlock is structurally impossible, and the compiler can check for cycles.
3. The order is fully fixed by the source and reproducible on every run (deterministic concurrency). The runtime may have mechanisms (stamp counting, sleep/wakeup — OS primitives), but they make no ordering decisions.
4. Data moves along the chain by handoff: after sending, the original is void. "Writing back through a copy" is structurally unwritable, so merge conflicts cannot exist.
5. Private work is not stamped: each thread producing and processing in its own region runs in parallel for free. Stamping private steps would weld the whole pipeline into serial.
6. Cross-thread loops are rendezvous-driven: whether the loop continues is decided by a signal sent back through the watermark chain (stored in a val, read at the loop head). Streaming with conditional termination and unknown totals works with constant memory. The stop protocol is a sentinel handoff.
7. Numbering machines (ticket-style) are user-level code — the language only provides the static watermark.

## Why

- Deterministic memory requires a computable execution flow: shared mutable state must be limited to explicit channels, or reclamation points stop being computable. Safe determinism and free shared-memory parallelism trade off — this design takes the former and deliberately cuts shared-memory parallelism (confirmed after challenge).
- Runtime numbering was rejected: order decided by arrival timing violates "order fixed by the source".
- Ownership with runtime blocking was rejected: it gives back both determinism and deadlock freedom.
- An unsafe shared-memory channel is not built in: the contention and merge problems come back with it.
- Honest costs, written down: a mismatched loop count is a dead wait the compiler cannot catch (inherent to streaming); the rendezvous loop is lock-step with no compute overlap — multi-core overlap comes from batching or manual double-buffering, both static and mechanism-free.

## Explicitly not provided (native; not a rejection — unsafe and FFI are the general escape hatches, and self-built means outside the determinism guarantee)

| Not provided natively | Self-built via |
|---|---|
| Runtime numbering | a user-level numbering machine |
| Locks / mutexes | unsafe spinlocks, or pthread through FFI |
| unsafe shared-memory channels | two threads reading and writing the same address |
| Atomic primitives | FFI to a C atomics library |
| Channel-close primitive | sentinel handoff protocol |
