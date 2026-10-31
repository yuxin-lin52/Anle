# 03 Statements

Status: implemented.

## Control flow

`if` / `else` / `for` / `while`, ended by a lone `;` on its own line — indentation is purely cosmetic and can never change meaning; a mismatched `;` is a loud parse error. `else:` must directly follow the `;` that closes its `if` body.

- Conditions must be booleans — `if 5:` is rejected.
- `for i in 1~5:` walks the inclusive range; `for x in collection:` iterates containers and strings.
- `while cond:` loops freely; `while cond max 100:` hard-caps iterations (a stall guard, not a ban on infinite loops).
- Loop variables are assignable locals; `max` requires an integer literal.

## Match

```anle
match call fn "safe-div"(10, 2):
case Ok("v"):
;1 print(call val "v")
;
case Err("reason"):
;1 print(call val "reason")
;
;
```

- `case Variant(binding)` destructures the payload; `case _:` is the fallback.
- The match subject accepts any expression with an inferable type.
- Exhaustiveness is checked at compile time — adding a variant exposes every stale match.
- Statement form (as above) and expression form (inside fn bodies) both exist.

## Assignment and re-binding

- Re-binding: `val "名" = new-value`.
- Delta: `val "次" = val "次" (+1)` — applies the parenthesized expression with the current value as the left operand; requires a current value.
- Index and field writes: `list[1] = v`, `head.next.value = 90` — write positions stay bare.

## kill

Inside `unsafe` blocks: `kill #:block` reclaims the target block's active regions; `#: block d kill` cleans the block itself on exit. Sequential statement position only; the target must not be the current block and must have been called by this chain. The compiler tracks every handle and reference-slot propagation path — a live reference at the kill point is a compile error listing all violations. See [08](08-memory.md).

## Why

- `;` termination with cosmetic indentation: pure indentation fails silently — wrong indentation changes meaning. A missing `;` is a loud parse error.
- Truthy conditions rejected: treating numbers as booleans is implicit conversion, and implicit conversion is where bugs hide.
- `max`: infinite loops are legal; the cap is a stall guard.
- Exhaustive match: variants are a closed set; adding one exposes every stale match — a compile-time guarantee, not a convention.
- `kill` uses tracking, not silence: failure is a compile error enumerating every violating reference. Loud beats quiet.
