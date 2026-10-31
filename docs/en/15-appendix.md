# 15 Appendices

Status: reference.

## Keywords

`add call fn val type fix give take outs if else for while in max match case unsafe pub sle ABI hide kill asm and or not null re fa int float bool str int_8 int_16 int_32 int_64 uint_8 uint_16 uint_32 uint_64 list map que stack`

## Operators and symbols

`= : ; ~ , . ( ) [ ] { } + - * / % < > ? & | ^ ! == != <= >= << >>` and the markers `#:` (block), `##:` (module), `###:` (package).

## Builtins (~40)

| Facet | Builtins |
|---|---|
| Containers | `len` `len_utf8` `push` `pop` `insert` `remove` `clear` `has` `empty` `sort` `index` `put` `keys` `vals` |
| Console | `print` |
| Files | `read_file` `save_file` |
| Input | `read_line` |
| Process | `exit` `args` `run` |
| Time / random | `sle_ms` `rnd` |
| unsafe / MMIO | `read_8/16/32/64` `write_8/16/32/64` |
| unsafe / C interop | `str_to_c` `str_from_c` |

Conversions are type names as functions: `int()` `float()` `str()` `bool()` and the fixed-width family — they are not builtin names.

## Type table

| Kind | Types |
|---|---|
| Scalars | `int` `float` `bool` `str` |
| Fixed-width (unsafe only) | `int_8/16/32/64` `uint_8/16/32/64` |
| Containers | `list(T)` `map(K, V)` `que(T)` `stack(T)` |
| Booleans | `re` `fa` |
| Empty | `null` |
| Handles | `Type*` (8-byte, shared) |

## Error codes

Compile-time: `E0` lexical / `E1` syntax / `E2` check / `E3` IR / `E4` backend — by stage; `E5` project config / `E6` environment / `E7` compiler internals / `E8` reserved / `E9` fallback — by ownership. Runtime guards: `R01` dangling handle, `R02` division by zero, `R03` overflow, `R04` out of bounds, `R05` missing key, `R06` out of memory, `R07` while over max.
