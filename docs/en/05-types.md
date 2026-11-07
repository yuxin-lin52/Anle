# 05 Types

Status: implemented. Capability constraints are designed but not yet implemented.

## Capability constraints

Capability annotations let generic code use operators on type parameters. They are declared with operators in the parameter list: `add fn "bigger"(T <) (a T, b T) T = (a > b) ? a : b`. Derivation: `>` `>=` derive from `<`; `!=` derives from `==`. Two-point checking: the definition point rejects operators outside the declared capabilities (errors point at the definition line); the instantiation point re-checks everything with the real type substituted. Until this lands, non-structural operations on type parameters are rejected outright.

## How it works

- Scalars: `int` (i64), `float`, `bool`, `str`. Variable types may be inferred; **function parameters and return types are mandatory**. Booleans are `re` / `fa`.
- Fixed-width integers `int_8/16/32/64`, `uint_8/16/32/64` are visible only in `unsafe`. Literals narrow automatically (`b & 0x0F`); variables need explicit casts. Safe-level overflow is an error; `unsafe` allows wraparound.
- Indices start at 1: `list[1]` is the first element. This matches the inclusive range operator — `for i in 1~length` walks the whole list, and there is never an "off by one" decision to make. Slices (`list[1~3]`, `s[2~4]`) reuse the same operator.
- `null` is the single empty value. Only positions explicitly declared nullable may hold it, and uses must be checked first. Field chains auto-dereference with per-level checks (`head.next.value`); a dangling reference is a runtime error with a line number.
- Conversions are type names as functions: `int("5")`, `str(number)`, `float(text)`, `uint8(300)`.
- Product types: `add type "Point" = (x int, y int)`. The type name is the constructor (`Point(3, 4)`); fields use `.`. There are no methods — operations are builtin functions, custom comparison is a comparator fn.
- Handles `Type*`: a starred type stores an 8-byte handle. Assignment, parameter passing, and containers copy the handle — all refer to one object. Unstarred types are value copies. Self-reference and mutual reference must be starred (unstarred cycles are rejected at compile time). Field chains auto-dereference. `list(Node*)` is allowed; map keys reject handles.
- Sum types: `add type "Expr" = Num(int) | Add(Expr*, Expr*)`. Variant names are constructors. Matching is `match` + `case Variant(binding)` with destructuring, a compile-time exhaustiveness check (adding a variant exposes every stale match), and `case _` as fallback. The match subject accepts any expression with an inferable type.
- Overflow: safe-level integer overflow is an error (constant overflow caught at compile time, runtime overflow caught with a line number); `unsafe` allows wraparound.
- Generics: compile-time monomorphization. `add type "Pair"(A, B) = (first A, second B)`; `add fn "swap"(A, B) (p Pair(A, B)) Pair(B, A)` — type arguments are inferred from the call site, and each used combination is expanded exactly once. By default only structural operations are allowed on type parameters; arithmetic requires a capability annotation (see "Capability constraints" above).

## Why

- Types mandatory at function boundaries: functions are the interface, and written types expose errors at call sites. Inferred types everywhere else keep the zero-burden default.
- 1-based indexing matches the inclusive range operator and `for` loops. Index, range, and loop share one starting point — there is never a "subtract one" decision.
- Explicit handles: a reference is a semantic choice, so it is written on the type where it is visible and checkable. Unstarred stays value semantics. Unstarred cycles are rejected, forcing the design to state "this is a reference".
- Sum types with exhaustive match: variants are a closed set; adding one exposes every match that does not handle it. That is a compile-time guarantee, not a convention.
- 8-byte payload slots everywhere: the same rule as container slots. Bare structs cannot enter variants or containers — the alternative is silent data corruption from mismatched slot widths.
- Monomorphization is the only viable route: type erasure needs runtime boxes and vtables, which conflict with having no runtime.
- Explicit conversions: where a number came from and what type it is stays written down. Implicit promotion turns a type mismatch into a runtime surprise.

## Not provided

- Methods — operations are builtin functions; custom types have fields only
- Operator overloading — custom comparison goes through comparator fns
- Auto-boxing — recursive fields must say `Type*`
- Nested containers `list(list(int))` — "unnamed places do not nest unnamed places"; group via map values or struct fields
- Implicit numeric promotion
