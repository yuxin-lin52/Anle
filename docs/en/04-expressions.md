# 04 Expressions

Status: implemented.

## Operators

| Family | Operators | Notes |
|---|---|---|
| Arithmetic | `+ - * / %` | `int` and `float` do not mix without an explicit cast; safe-level overflow is an error |
| Comparison | `== != < > <= >=` | value comparison; `==` descends into strings, lists, maps, and struct fields |
| Logic | `and` `or` `not` | keywords; `and`/`or` short-circuit |
| Bitwise | `& \| ^ << >> !` | integers only; `!` is bitwise NOT (`not` is logical NOT); `>>` is arithmetic shift |
| Ternary | `cond ? a : b` | lowest precedence, right-assoc, lazy — the unchosen branch never runs |

Precedence: comparison `< | < ^ < & < shift < add < multiply`.

## Calls and reads

- `call val "name"` reads a value; `call fn "f"(x)` calls a function.
- Field access: `position.x`; chains auto-dereference handles with per-level null checks (`head.next.value`).
- Indexing: `list[1]`, `map["key"]` — 1-based, matching the inclusive range operator.
- Slices: `list[1~3]`, `s[2~4]` — 1-based, both ends inclusive, reusing the range operator.

## Conversions

Type names as functions: `int("5")`, `str(number)`, `float(text)`, `uint8(300)`. There are no implicit conversions — `int` and `float` do not mix without one, and where a number came from stays written down.

## Why

- Lazy ternary: it is the only exit for recursive formula bodies — under eager evaluation a recursive call in an expression would expand forever.
- No chained comparisons: parsing complexity outweighs the use case; write `and`.
- Explicit conversions: implicit promotion turns a type mismatch into a runtime surprise.
- `!` for bitwise NOT and `not` for logical NOT: `~` is already the range operator, `!` was free, and both are familiar.
