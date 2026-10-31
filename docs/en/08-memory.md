# 08 Memory: the region tree

Status: implemented.

## How it works

- Memory is allocated per block instance. When a block's region dies, everything in it dies with it; reclamation is O(1) per region, not per object.
- Regions form a tree: a region hangs under its caller's region. Block exit does not reclaim — the region lives as long as the call chain (a child dies when its parent dies). The chain root's exit reclaims the whole tree.
- Reclamation points are fixed at compile time from the block reference graph. At run time there is no GC thread, no stop-the-world pause, and no reference counting.
- A region pool (64KB uniform blocks plus a large-block path) recycles dead regions; unused regions are returned to the pool and reused by the next same-size allocation.
- Loop-level reclamation: sub-regions created by calls inside a loop body are returned in bulk at the end of each iteration. This is compiler-implicit, with a conservative analysis (enabled only when nothing escapes the loop).
- `kill` (inside `unsafe` only): the statement `kill #:block` reclaims all active regions of the target block and its subtree; the block-close suffix `#: block d kill` cleans the block itself on exit. Constraints: sequential statement position only; the target must not be the current block and must have been called by this chain. The compiler tracks every handle and reference-slot propagation path — a live reference at the kill point is a compile error listing all violations.

## Why

- Regions are logically a tree (siblings are independent); the previous stack implementation was LIFO — sibling-independent reclamation and early release were both impossible. That was the defect being fixed.
- Compile-time reclamation points are the premise, not an option: no GC thread, no pauses, no counters, and cycle-freedom all follow from it.
- Prior art: region-inference systems still nest regions on a stack (sibling independence fails); explicit region schemes put handles into the type system and leave the heap to a runtime recycler; ownership is per-value and its checks burden the programmer; reference counting costs runtime work and leaks cycles. None fit.
- Block layout affects memory footprint, never correctness — reclamation granularity is a performance knob, not a requirement. Hence no block-size rules and no compiler nagging.
- `kill` uses tracking, not silence: failure is a compile error enumerating every violating reference. Loud beats quiet.

## Not provided

- Region compaction / relocation (it would invalidate handles; fragmentation is absorbed by the 64KB pool)
- Compiler-initiated early reclamation (the compiler does not make performance decisions for the developer)
- Persistent blocks / `promote` — a separate topic
- Cross-process regions — the region tree is single-process, single-chain
