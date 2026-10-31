# Anle

[中文](README.zh-CN.md) | English

Anle is an ahead-of-time compiled language. Memory is managed by the compiler: reclamation points are computed from the program's block structure, and the runtime has no garbage collector, no pauses, and no per-object counting.

## Design motivation

Anle is built on a single division of labor: **simple to write, complex to implement.**

Every language must answer one question: who is responsible for memory? Manual management hands correctness to the programmer. Runtime management removes that work, but adds an extra burden and runtime pauses. Compiler-checked ownership guarantees correctness, at the price of a set of rules the programmer must learn and satisfy.

Anle takes none of these trade-offs. The program is structured as blocks; values live in the block that declares them; all data crossing a block boundary goes through an explicit channel. Given that structure, the compiler computes the lifetime and reclamation point of every value. The programmer writes blocks and states data flow; the compiler does the rest.

The same division runs through the whole language. The syntax is small and regular. Direct memory access and explicit early reclamation (`kill`) exist only inside `unsafe` blocks. Anle has no manual memory allocation at all — as a consequence, the minimal runtime environment always ships with a memory-management base, and Anle provides inline assembly and C ABI interoperability for environments that need their own.

## Language overview

```anle
#: main b
    add val "total" int = 0
    add val "i" int = 1
    while call val "i" <= 100:
        val "total" = call val "total" + call val "i"
        val "i" = call val "i" + 1
    ;
    print(call val "total")
#: main d
```

This prints `5050`. The rules visible in this example:

* All code lives in blocks. A block opens with `#: name b` (born) and closes with `#: name d` (death). Blocks are flat — they do not nest.
* Names are created with `add` and re-bound with `val "name" =` (`add` is never repeated).
* Values are read as `call val "name"`. There are no bare identifiers: every read states its kind, and every cross-block reference states its source.
* No implicit conversions. `int` and `float` do not mix without an explicit cast.
* Conditions must be booleans — `if 5:` is a compile error.
* Indices start at 1, matching the inclusive range operator (`for i in 1\~5` covers 1 through 5).
* Control-flow bodies are terminated by a lone `;` on its own line. Indentation is purely cosmetic and can never change meaning; a mismatched `;` is a loud parse error.

## Memory model

* Memory is allocated per block instance. When a block's region dies, everything in it dies with it; reclamation is O(1) per region, not per object.
* Reclamation points are fixed at compile time from the block reference graph. At run time there is no GC thread, no stop-the-world pause, and no reference counting.
* Data crossing a block boundary takes one of two explicit forms: a **reference slot** (`call #:block val "name"` — reads see the freshest value, writes go through) or a **deep copy** (`take #:block val "name"` — the value at that moment belongs to the caller).
* Reference cycles do not leak, because nothing is reclaimed object-by-object.

The model gives every programmer the same simple default: put the code in blocks, and memory takes care of itself — a beginner can write an entire program in one block with zero memory burden. The block layout never affects correctness, only memory footprint. For memory- or performance-sensitive projects, advanced developers can tune it: one step per block, plus `kill` inside `unsafe`, to keep the memory peak low.

## Building

The compiler source is in [`compiler/`](compiler) — a Rust workspace of nine crates.

```
cd compiler
cargo build --release --no-default-features --features backend   # C backend only, no LLVM needed
cargo build --release                                            # both backends, needs LLVM 18.1
```

The C backend lowers the intermediate representation to C source text and hands it to `clang` (path from `ANLE_CC`, otherwise `clang` on `PATH`). The LLVM backend links against `LLVM-C.dll`, so `LLVM_SYS_181_PREFIX` must point at an LLVM 18.1 installation.

```
./target/release/anlec build demos/en/physics-engine/physics-engine
```

## Implementation status

* The compiler is implemented in Rust; the source is included in this repository. Self-hosting is in an early stage: the lexer of the self-hosted front end is complete and passes token-for-token comparison against the host implementation.
* Two backends are implemented, **C** and **LLVM**; both are compiled through clang, and the same source compiled through both backends produces byte-identical program output. LLVM is planned to become the primary backend.
* Performance is currently about 4x behind an equivalent C program. The causes are identified — per-statement source-position bookkeeping and handle indirection — and fixes are planned.
* Known gaps: parameter parsing for generic functions is incomplete; some diagnostic wording does not match the actual cause.
* The syntax is **not frozen**. Breaking changes are expected; anything written against this language will need updating.
* The standard library is currently limited to about 40 builtin functions; an upper layer written in Anle is planned.

## Verification

The project's acceptance criterion is program behavior, not unit-test counts. Example, from the physics-engine demo in [demos/en/physics-engine/](demos/en/physics-engine/physics-engine.an):

* 400 particles, 400 simulation steps, roughly 32 million pair checks.
* The program self-checks four invariants after the run (elastic exchange, momentum conservation, wall bouncing, containment); the recorded output:

```
collision exchange: pass
momentum conserved: pass
wall bounce: pass
containment: pass
benchmark done, collisions: 7343
```

* The collision count, 7343, matches reference implementations written in C and Rust exactly.
* Both compiler backends produce byte-identical output; three consecutive runs are identical.

## Diagnostics

Diagnostics carry four parts: what happened, the source line, what the compiler expected, and how to fix it. They are currently emitted in Chinese; localization is on the roadmap. A real example:

```
\[E202] 位于 例.an 第 2 行第 1 列的「赋值」发生错误，
源码为：add val "v" float = 0 - 5，
错误为：「v」声明是 float，右边是 int，
编译器预期为：右边的值应当是 float 型（左边声明说了算）。
改法：改成 float 的值，或把声明改成 int。
```

## Feedback

The repository is public to collect design feedback. Issues are open — design questions, rough edges in the examples, and disagreements with the choices above are all welcome. The [design documentation](docs/README.md#english) covers the decisions behind this overview.

## License

Apache 2.0. See [LICENSE](LICENSE).

