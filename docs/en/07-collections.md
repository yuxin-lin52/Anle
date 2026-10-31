# 04 Collections

Status: implemented.

## How it works

Four kinds: `list`, `map`, `que` (FIFO), `stack` (LIFO). Element types use parentheses: `list(int)`, `map(str, int)`.

| Operation | Function | Applies to |
|---|---|---|
| Length | `len(x)` (str counts characters) / `len_utf8` (bytes) | all |
| Append / take last | `push` / `pop` | list, que, stack |
| Insert / remove at position | `insert` / `remove` | list (`remove` deletes by map key) |
| Clear | `clear` | all |
| Contains | `has` | all |
| Keys / values | `keys` / `vals` | map |
| Sort / find position | `sort` / `index` | list |
| Empty check | `empty` | all |
| Put key-value | `put` | map |

- Access is by index (`list[1]`, `config["timeout"]`); assignment is direct (`list[1] = v`). Slices reuse the range operator: `list[1~3]`, `s[2~4]` — 1-based, both ends inclusive.
- Construction: `list` and `map` have literals (`[1, 2, 3]`, `{"timeout": 30}`); `que`/`stack` construct via the type name (`que([1,2,3])`, `stack()`); an empty map is `map()`.
- Iteration: `for x in collection:`; strings iterate by character.
- Nesting rule: a list/que/stack element cannot hold a container; a map value or a struct field can. The principle: unnamed places do not nest unnamed places. Rejections come with a fix: group through map values (`{"语文": [90, 85]}`) or struct fields.
- `print` output matches the source form: `[1, 2, 3]`, `que[1, 2]`, `{"timeout": 30}`, `Point(3, 4)` — the same syntax as construction, zero new symbols.
- Naming rule: the name encodes the type when semantics differ per type (`len` characters vs `len_utf8` bytes; `read_8/16/32/64`); builtin dispatch is used when one action is reused across types (`str(x)` accepts int/float/bool).

## Why

- Builtin-function style: no "method" concept is introduced — custom types have fields only.
- One `push`/`pop` serves list, que, and stack: reuse saves memory budget in the user's head.
- `put`, not `set`: `set` collides with assignment `=`. Map access goes through the index, the same as list — learn once.
- Map literals: `"name": value` reads at a glance. Inside `{}` the `:` is a separate context and never collides with block markers or `if …:`.
- The nesting ban: mismatched slot widths silently corrupted data. The current rule rejects loudly, with the grouping fix written into the error.
- Output equals source: what `print` shows can be typed back as a literal — zero memory in either direction.

## Not provided

- Method syntax
- `set` (+1 keyword if ever needed)
- Multi-line map literals (entangled with `;` termination rules)
- Nested containers — flat storage plus manual indexing; reconsidered if real projects hit the wall
