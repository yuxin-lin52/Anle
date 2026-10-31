# 11 Modules

Status: implemented.

## How it works

- The compilation unit is a directory: `anlec build <目录>`. The entry is fixed at the directory root's `main.an` — zero configuration. The boundary is the directory given on the command line; the compiler never searches upward for a project root.
- One file is one module. The module name is the directory path plus the file name without its extension, with directory separators mapped to `.` (`frontend/tokenize.an` → module `frontend.tokenize`). Path is namespace. Moving a file is renaming it (an accepted cost).
- The dependency closure is collected by scanning `##:module` prefixes recursively. Each module compiles once; circular references are allowed (harmless under combined-source compilation); an `.an` file referenced by nothing is not compiled.
- Combined-source compilation: the closure merges into one IR and one executable, invisible to the user.
- Visibility: cross-module calls may only target `pub` blocks (private by default). `hide` removes individual items from the public surface. The boundary is cross-module only — other blocks in the same module can still call hidden items.
- Type names are globally unique, including sum-type variant names, even across modules. The rationale: with no import scope and bare-name constructors, a bare name must be unique to be referenceable.
- Diagnostics are attributed to the real source file — an error in a library file reports that file's name and line, not `main.an`.
- Block names are unique within a module; across modules the same name is allowed — that is what `##:module` disambiguates.

## Why

- Scope drawn first: v0.x only solves splitting one program across files on one machine. Package management, incremental compilation, and version locking are explicitly out — ecosystem and caching are later steps.
- Module = file, path = namespace: order at scale. The cost (moving = renaming) trades against "order for large projects over minimal-implementation convenience".
- Explicit compilation boundary: auto-searching upward for a project root ends at the disk root at worst. The command line names the directory — one rule, zero ambiguity.
- Distribution is a plain zip: renamed-suffix archives (.anp) are hostile under Windows hidden extensions; the real application format (with a manifest) arrives with the v1 block layer as a separate format, not a renamed zip.
- Circular references allowed: harmless under combined-source compilation.
- Tail-name fallback: `##:工具` matches a unique `lib/工具.an`; a collision among several modules is a loud ambiguity error listing the candidates.

## Standard library

Two layers:

- **The base**: the OS's last centimeter as a thin builtin layer, plus a stable runtime symbol contract. The contract is backend-independent — the C backend links `anle_rt.c`, the LLVM backend links the same symbols, and any future backend follows the same rule. The current base (~40 builtins) is frozen.
- **The upper layer**: pure Anle source, fully backend-independent, introduced through the module system. API shape: module-prefixed functions (`字符串.分割(串, 分隔)`).

First batch (all zero-base-dependency, pure Anle): string five (split, trim, find-and-replace, case conversion, search); container algorithms three (map, filter, fold — generic free functions, the transform logic is a fn parameter); generic wrappers two (`Result(T, E)`, `Optional(T)`).

## Not provided

- Package management / dependency download / version locking — v1 ecosystem topics (package coordinates `lib.作者.仓库`, the `lib.anle` manifest, the `api.anle` interface listing, and the `###:` three-level prefix are already designed)
- Incremental compilation — compilation is milliseconds to seconds; module-level incrementality is the prerequisite for real incremental builds
- Environment injection / registry / installer services — see the SDK packaging design
